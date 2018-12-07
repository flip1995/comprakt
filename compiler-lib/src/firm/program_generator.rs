use super::{ty_from_checked_type, Class, Field, Method, MethodBodyGenerator, Program, Runtime};
use crate::{
    ast,
    strtab::{StringTable, Symbol},
    type_checking::{
        type_analysis::TypeAnalysis,
        type_system::{ClassMethodBody, TypeSystem},
    },
    visitor::NodeKind,
};
use libfirm_rs::{bindings::*, *};
use log;
use std::{cell::RefCell, collections::HashMap, ffi::CString, rc::Rc};

pub struct ProgramGenerator<'src, 'ast> {
    runtime: Runtime,
    type_system: &'src TypeSystem<'src, 'ast>,
    type_analysis: &'src TypeAnalysis<'src, 'ast>,
    strtab: &'src mut StringTable<'src>,
}

impl<'src, 'ast> ProgramGenerator<'src, 'ast> {
    pub fn new(
        type_system: &'src TypeSystem<'src, 'ast>,
        type_analysis: &'src TypeAnalysis<'src, 'ast>,
        strtab: &'src mut StringTable<'src>,
    ) -> Self {
        Self {
            runtime: Runtime::new(),
            type_system,
            type_analysis,
            strtab,
        }
    }

    pub fn generate(mut self) -> Program<'src, 'ast> {
        let classes = self.build_entities();

        // TODO glue classes and runtime functions together here!
        for class in classes.values() {
            let class = class.borrow();
            log::debug!("generate methods for class {:?}", class.def.name);
            for method in class.methods.values() {
                log::debug!("generate method body for {:?}", method.borrow().def.name);

                let matured_graph = self.generate_method_body(&method.borrow(), &class, &classes);
                // TODO assert matured
                method.borrow_mut().graph = Some(matured_graph);
            }
        }
        Program { classes }
    }

    fn generate_method_body(
        &mut self,
        method: &Method<'src, 'ast>,
        own_class: &Class<'src, 'ast>,
        classes: &HashMap<Symbol<'src>, Rc<RefCell<Class<'src, 'ast>>>>,
    ) -> Graph {
        match method.body {
            ClassMethodBody::Builtin(builtin) => {
                self.runtime.graph_from_builtin_method_body(builtin)
            }
            ClassMethodBody::AST(body) => {
                let mut local_var_def_visitor = LocalVarDefVisitor::new();
                local_var_def_visitor.visit(&NodeKind::from(body));

                assert!(!method.def.is_static || (method.def.is_static && method.def.is_main));
                let this_param = if method.def.is_main { 0 } else { 1 };
                let param_count =
                    this_param + method.def.params.len() + local_var_def_visitor.count;
                let graph = Graph::function_with_entity(method.entity, param_count);
                let mut method_body_gen = MethodBodyGenerator::new(
                    graph,
                    own_class,
                    classes,
                    Rc::clone(&method.def),
                    &self.type_analysis,
                    &self.runtime,
                    &mut self.strtab,
                );
                method_body_gen.gen_method(body);
                unsafe {
                    irg_finalize_cons(graph.into());
                }
                graph
            }
        }
    }

    fn build_entities(&self) -> HashMap<Symbol<'src>, Rc<RefCell<Class<'src, 'ast>>>> {
        let mut classes = HashMap::new();
        // Define classes
        for class in self.type_system.defined_classes.values() {
            unsafe {
                log::debug!("gen class {:?}", class.name.as_str());
                let class_name_str = class.name.as_str();
                let class_name = CString::new(class_name_str).unwrap();
                let class_name_id = new_id_from_str(class_name.as_ptr());
                let class_type = new_type_class(class_name_id);
                let class_entity = Entity::new_global(&class_name, class_type.into());

                let gclass = Rc::new(RefCell::new(Class {
                    def: class,
                    name: class_name,
                    entity: class_entity,
                    fields: HashMap::new(),
                    methods: HashMap::new(),
                }));

                for field in class.iter_fields() {
                    log::debug!("\tgen field {:?}", field.name.as_str());
                    let field_type = ty_from_checked_type(&field.ty)
                        .expect("field type must be convertible to a Firm type");
                    let field_name =
                        CString::new(format!("{}.F.{}", class_name_str, field.name)).unwrap();
                    let field_entity = new_entity(
                        class_type,
                        field_name.as_ptr() as *mut i8,
                        field_type.into(),
                    );

                    gclass.borrow_mut().fields.insert(
                        field.name,
                        Rc::new(RefCell::new(Field {
                            _class: Rc::downgrade(&gclass),
                            _name: field_name,
                            def: field,
                            entity: field_entity.into(),
                        })),
                    );
                }

                for method in class.iter_methods() {
                    log::debug!("\tgen method{:?}", method.name.as_str());
                    assert!(!method.is_static || (method.is_static && method.is_main));
                    let mut method_type = FunctionType::new();

                    // add this parameter
                    if !method.is_main {
                        method_type.add_param(PrimitiveType::ptr());
                    }

                    for param in &method.params {
                        let param_type = ty_from_checked_type(&param.ty)
                            .expect("parameter must be convertible to a Firm type");
                        method_type.add_param(param_type);
                    }
                    if let Some(return_ty) = ty_from_checked_type(&method.return_ty) {
                        method_type.set_res(return_ty);
                    }
                    let method_type = method_type.build(!method.is_main);

                    let method_name = if method.is_main {
                        CString::new("mj_main").unwrap()
                    } else {
                        CString::new(format!("{}.M.{}", class_name_str, method.name)).unwrap()
                    };
                    let method_entity = new_entity(
                        class_type,
                        method_name.as_ptr() as *mut i8,
                        method_type.into(),
                    );

                    gclass.borrow_mut().methods.insert(
                        method.name,
                        Rc::new(RefCell::new(Method {
                            _class: Rc::downgrade(&gclass),
                            _name: method_name,
                            def: Rc::clone(&method),
                            entity: method_entity.into(),
                            graph: None,
                            body: method.body,
                        })),
                    );
                }

                default_layout_compound_type(class_type);

                classes.insert(class.name, gclass);
            }
        }

        classes
    }
}

struct LocalVarDefVisitor {
    count: usize,
}

impl<'a, 'f> LocalVarDefVisitor {
    fn new() -> Self {
        Self { count: 0 }
    }

    fn visit(&mut self, node: &NodeKind<'a, 'f>) {
        use self::NodeKind::*;
        node.for_each_child(&mut |child| {
            if let Stmt(stmt) = child {
                if let ast::Stmt::LocalVariableDeclaration(..) = stmt.data {
                    self.count += 1;
                }
            }

            self.visit(&child)
        });
    }
}