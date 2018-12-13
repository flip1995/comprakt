use crate::{
    entity::Entity,
    nodes_gen::{self, Block, Node, NodeFactory, Phi, Proj, ProjKind},
    tarval::Tarval,
};
use libfirm_rs_bindings as bindings;
use std::{
    fmt,
    hash::{Hash, Hasher},
};

macro_rules! simple_node_iterator {
    ($iter_name: ident, $len_fn: ident, $get_fn: ident, $id_type: ty) => {
        pub struct $iter_name {
            node: *mut bindings::ir_node,
            cur: $id_type,
            len: $id_type,
        }

        impl $iter_name {
            fn new(node: *mut bindings::ir_node) -> Self {
                Self {
                    node,
                    len: unsafe { bindings::$len_fn(node) },
                    cur: 0,
                }
            }
        }

        impl Iterator for $iter_name {
            type Item = Node;

            fn next(&mut self) -> Option<Node> {
                if self.cur == self.len {
                    None
                } else {
                    let out = unsafe { bindings::$get_fn(self.node, self.cur) };
                    self.cur += 1;
                    Some(NodeFactory::node(out))
                }
            }
        }

        impl ExactSizeIterator for $iter_name {
            fn len(&self) -> usize {
                self.len as usize
            }
        }
    };
}

impl Block {
    pub fn keep_alive(self) {
        unsafe { bindings::keep_alive(self.internal_ir_node()) }
    }

    pub fn num_cfgpreds(self) -> i32 {
        unsafe { bindings::get_Block_n_cfgpreds(self.internal_ir_node()) }
    }
}

impl Phi {
    pub fn phi_preds(self) -> PhiPredsIterator {
        PhiPredsIterator::new(self.internal_ir_node())
    }
}

simple_node_iterator!(PhiPredsIterator, get_Phi_n_preds, get_Phi_pred, i32);

impl Proj {
    pub fn proj(self, num: u32, mode: bindings::mode::Type) -> Proj {
        Proj::new(unsafe { bindings::new_r_Proj(self.internal_ir_node(), mode, num) })
    }

    pub fn kind(self) -> ProjKind {
        NodeFactory::proj_kind(self)
    }
}

/// A trait to abstract from Node enum and various *-Node structs.
pub trait NodeTrait {
    fn internal_ir_node(&self) -> *mut bindings::ir_node;

    fn mode(&self) -> bindings::mode::Type {
        unsafe { bindings::get_irn_mode(self.internal_ir_node()) }
    }

    fn block(&self) -> Block {
        let block_ir_node = unsafe { bindings::get_nodes_block(self.internal_ir_node()) };
        match NodeFactory::node(block_ir_node) {
            Node::Block(block) => block,
            _ => panic!("Expected block."),
        }
    }

    fn out_nodes(&self) -> OutNodeIterator {
        OutNodeIterator::new(self.internal_ir_node())
    }

    fn in_nodes(&self) -> InNodeIterator {
        InNodeIterator::new(self.internal_ir_node())
    }

    fn node_id(&self) -> i64 {
        unsafe { bindings::get_irn_node_nr(self.internal_ir_node()) }
    }

    // TODO autogenerate
    fn is_block(&self) -> bool {
        unsafe { bindings::is_Block(self.internal_ir_node()) != 0 }
    }

    // TODO autogenerate
    fn is_jmp(&self) -> bool {
        unsafe { bindings::is_Jmp(self.internal_ir_node()) != 0 }
    }

    fn is_const(&self) -> bool {
        unsafe { bindings::is_Const(self.internal_ir_node()) != 0 }
    }

    // TODO implement methods from
    // https://github.com/libfirm/jFirm/blob/master/src/firm/nodes/Node.java
}

simple_node_iterator!(InNodeIterator, get_irn_arity, get_irn_n, i32);

// TODO: should we use dynamic reverse edges instead of reverse
simple_node_iterator!(OutNodeIterator, get_irn_n_outs, get_irn_out, u32);

#[allow(clippy::derive_hash_xor_eq)]
impl Hash for Node {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // k1 == k2 => hash(k1) == hash(k2)
        // has to hold, update PartialEq implementation if this code
        // is updated.
        self.internal_ir_node().hash(state);
    }
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        // k1 == k2 => hash(k1) == hash(k2)
        // has to hold, update Hash implementation if this code
        // is updated.
        self.internal_ir_node() == other.internal_ir_node()
    }
}

impl Eq for Node {}

impl From<Node> for *mut bindings::ir_node {
    fn from(n: Node) -> *mut bindings::ir_node {
        n.internal_ir_node()
    }
}

impl From<Box<dyn ValueNode>> for Node {
    fn from(n: Box<ValueNode>) -> Node {
        NodeFactory::node(n.internal_ir_node())
    }
}

impl From<&Box<dyn ValueNode>> for Node {
    fn from(n: &Box<ValueNode>) -> Node {
        NodeFactory::node(n.internal_ir_node())
    }
}

impl fmt::Debug for nodes_gen::Call {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Call to {:?} {}", self.ptr(), self.node_id())
    }
}

impl nodes_gen::Address {
    pub fn entity(self) -> Entity {
        unsafe { bindings::get_Address_entity(self.internal_ir_node()).into() }
    }

    pub fn set_entity(self, ir_entity: Entity) {
        unsafe {
            bindings::set_Address_entity(self.internal_ir_node(), ir_entity.into());
        }
    }
}

impl fmt::Debug for nodes_gen::Address {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Address of {:?} {}",
            self.entity().name_string(),
            self.node_id()
        )
    }
}

#[derive(Debug)]
pub struct DowncastErr(Node);

macro_rules! downcast_node {
    ($cast_name: ident, $trait_name: ident, [$($variant: ident),*]) => (
        pub fn $cast_name(node: Node) -> Result<Box<dyn $trait_name>, DowncastErr> {
            match node {
                $(
                    Node::$variant(node) => Ok(Box::new(node)),
                )*
                _ => Err(DowncastErr(node)),
            }
        }
    );
}

pub trait ValueNode: NodeTrait {
    fn value_nodes(&self) -> Vec<Box<dyn ValueNode>>;
    fn compute(&self, values: Vec<Tarval>) -> Tarval;
}

pub trait BinOp {
    fn left(&self) -> Box<dyn ValueNode>;
    fn right(&self) -> Box<dyn ValueNode>;
    fn compute(&self, left: Tarval, right: Tarval) -> Tarval;
}

impl ValueNode for Const {
    fn value_nodes(&self) -> Vec<Box<dyn ValueNode>> {
        vec![]
    }

    fn compute(&self, values: Vec<Tarval>) -> Tarval {
        assert!(values.len() == 0);
        self.tarval()
    }
}

impl ValueNode for Phi {
    fn value_nodes(&self) -> Vec<Box<dyn ValueNode>> {
        self.in_nodes()
            .map(|n| try_as_value_node(n).unwrap())
            .collect()
    }

    fn compute(&self, values: Vec<Tarval>) -> Tarval {
        values.iter().fold(Tarval::unknown(), |a, b| a.join(*b))
    }
}

impl ValueNode for Proj {
    fn value_nodes(&self) -> Vec<Box<dyn ValueNode>> {
        match self.kind() {
            ProjKind::Div_Res(_) => vec![try_as_value_node(self.pred()).unwrap()],
            ProjKind::Mod_Res(_) => vec![try_as_value_node(self.pred()).unwrap()],
            _ => vec![],
        }
    }

    fn compute(&self, values: Vec<Tarval>) -> Tarval {
        assert!(values.len() <= 1);
        if values.len() == 1 {
            values[0]
        } else {
            Tarval::bad()
        }
    }
}

macro_rules! binop_impl {
    ($node_ty: ident, $compute: expr) => {
        impl BinOp for $node_ty {
            fn left(&self) -> Box<dyn ValueNode> {
                try_as_value_node($node_ty::left(*self)).unwrap()
            }
            fn right(&self) -> Box<dyn ValueNode> {
                try_as_value_node($node_ty::right(*self)).unwrap()
            }
            fn compute(&self, left: Tarval, right: Tarval) -> Tarval {
                $compute(self, left, right)
            }
        }

        impl ValueNode for $node_ty {
            fn value_nodes(&self) -> Vec<Box<dyn ValueNode>> {
                vec![BinOp::left(self), BinOp::right(self)]
            }

            fn compute(&self, values: Vec<Tarval>) -> Tarval {
                assert!(values.len() == 2);
                BinOp::compute(self, values[0], values[1])
            }
        }
    };
}

use self::nodes_gen::{Add, Cmp, Const, Div, Eor, Mod, Mul, Sub};

binop_impl!(Add, |_n, l, r| l + r);
binop_impl!(Sub, |_n, l, r| l - r);
binop_impl!(Mul, |_n, l, r| l * r);
binop_impl!(Div, |_n, l, r| l / r);
binop_impl!(Mod, |_n, l, r| l % r);
binop_impl!(Eor, |_n, l, r| l ^ r);
binop_impl!(Cmp, |n: &Cmp, l: Tarval, r| l.lattice_cmp(n.relation(), r));

downcast_node!(try_as_bin_op, BinOp, [Add, Sub, Mul, Div, Mod, Eor, Cmp]);

pub trait UnaryOp {
    fn operand(&self) -> Box<dyn ValueNode>;
    fn compute(&self, val: Tarval) -> Tarval;
}

use self::nodes_gen::{Conv, Minus};

macro_rules! unaryop_impl {
    ($node_ty: ident, $compute: expr) => {
        impl UnaryOp for $node_ty {
            fn operand(&self) -> Box<dyn ValueNode> {
                try_as_value_node(self.op()).unwrap()
            }
            fn compute(&self, val: Tarval) -> Tarval {
                $compute(self, val)
            }
        }

        impl ValueNode for $node_ty {
            fn value_nodes(&self) -> Vec<Box<dyn ValueNode>> {
                vec![self.operand()]
            }

            fn compute(&self, values: Vec<Tarval>) -> Tarval {
                assert!(values.len() == 1);
                UnaryOp::compute(self, values[0])
            }
        }
    };
}

unaryop_impl!(Minus, |_n, val: Tarval| -val);
unaryop_impl!(Conv, |n: &Conv, val: Tarval| val
    .cast(n.mode())
    .unwrap_or_else(Tarval::bad));
downcast_node!(try_as_unary_op, UnaryOp, [Minus, Conv]);

downcast_node!(
    internal_try_as_value_node,
    ValueNode,
    [
        Const, Phi, // special
        Minus, Conv, // unary
        Add, Sub, Mul, Div, Mod, Eor, Cmp // binary
    ]
);

pub fn try_as_value_node(node: Node) -> Result<Box<dyn ValueNode>, DowncastErr> {
    match node {
        Node::Proj(node, _) => Ok(Box::new(node)),
        _ => internal_try_as_value_node(node),
    }
}
