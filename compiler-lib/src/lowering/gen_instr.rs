use super::{
    lir::{Allocator, *},
    lir_allocator::Ptr,
};
use itertools::Itertools;
use libfirm_rs::{
    nodes::{Node, NodeTrait, ProjKind},
    types::{Ty, TyTrait},
    Entity,
};
use std::{collections::HashMap, convert::TryInto};
use strum_macros::*;

#[derive(Debug, Clone, EnumDiscriminants)]
enum Computed {
    Void,
    Value(Ptr<MultiSlot>),
}
impl Computed {
    fn must_value(&self) -> Ptr<MultiSlot> {
        match self {
            Computed::Value(vs) => *vs,
            x => panic!(
                "computed must be a value, got {:?}",
                ComputedDiscriminants::from(x)
            ),
        }
    }
}

pub struct GenInstrBlock {
    code: Code,
    computed: HashMap<Node, Computed>,
}

impl GenInstrBlock {
    pub(super) fn fill_instrs(graph: &BlockGraph, mut block: Ptr<BasicBlock>, alloc: &Allocator) {
        let mut b = GenInstrBlock {
            code: Code::default(),
            computed: HashMap::new(),
        };
        b.gen(graph, block, alloc);
        let GenInstrBlock { code, .. } = b;
        block.code = code;
    }

    fn comment(&mut self, args: std::fmt::Arguments<'_>) {
        self.code
            .body
            .push(Instruction::Comment(format!("{}", args)));
    }

    fn gen(&mut self, graph: &BlockGraph, block: Ptr<BasicBlock>, alloc: &Allocator) {
        // LIR metadata dump + fill computed values
        for (num, multislot) in block.regs.iter().enumerate() {
            self.comment(format_args!("Slot {}:", num));

            // "Input slots"
            for edge in block.preds.iter() {
                edge.register_transitions
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, dst))| dst.num == num)
                    .filter(|(idx, _)| {
                        let must_copy_in_source = edge.must_copy_in_source(*idx);
                        assert!(
                            // "must_copy_in_source => !must_copy_in_target"
                            !must_copy_in_source || !edge.must_copy_in_target(*idx),
                            "possible lost-copy detected"
                        );

                        // We copy everything that doesn't have to be copied in `source` in `target`
                        // (this includes everything that *has* to be copied in `target`), as
                        // demonstrated by the above assertion
                        !must_copy_in_source
                    })
                    .for_each(|(_, (src, dst))| {
                        let (src, dst) = (*src, *dst);
                        self.code.copy_in.push(CopyPropagation { src, dst });
                    })
            }

            match &(**multislot) {
                MultiSlot::Single(slot) => self.comment(format_args!("\t=  {:?}", slot.firm)),
                MultiSlot::Multi { ref slots, .. } => {
                    for slot in slots {
                        self.comment(format_args!("\t=  {:?}", slot.firm));
                    }
                }
            }

            // "Output slots"
            for edge in block.succs.iter() {
                edge.register_transitions
                    .iter()
                    .enumerate()
                    .filter(|(_, (src, _))| src.num() == num)
                    .filter(|(idx, _)| edge.must_copy_in_source(*idx))
                    .for_each(|(_, (src, dst))| {
                        let (src, dst) = (*src, *dst);
                        self.code.copy_out.push(CopyPropagation { src, dst });
                    })
            }
        }

        // LIR conversion
        // We compute the values required by our successors.
        let values_to_compute = {
            let mut v: Vec<Node> = Vec::new();

            v.extend(
                block
                    .succs
                    .iter()
                    .flat_map(move |out_edge| {
                        out_edge
                            .register_transitions
                            .iter()
                            .map(|(src_slot, _)| src_slot.clone())
                            .collect::<Vec<_>>()
                    })
                    .map(|src_slot| src_slot.firm())
                    .dedup(),
            );

            // Apart from the values that _must_ be flown out, all leave
            // instructions must be computed to account for side-effects calls, etc.
            // => return_nodes, jmp_nodes, cond_nodes
            let leave_nodes = block
                .firm
                .out_nodes()
                .filter(|n| Node::is_return(*n) || Node::is_jmp(*n) || Node::is_cond(*n))
                .collect::<Vec<_>>();
            log::debug!("block {:?} leave nodes: {:?}", block.firm, leave_nodes);
            v.extend(leave_nodes);

            v
        };

        for out_value in values_to_compute {
            self.comment(format_args!("\tgen code for out_value={:?}", out_value));
            self.gen_value(graph, block, alloc, out_value);
        }
    }

    fn is_computed(&self, node: Node) -> bool {
        self.computed.contains_key(&node)
    }

    fn must_computed(&self, node: Node) -> &Computed {
        self.computed
            .get(&node)
            .unwrap_or_else(|| panic!("must have computed value for node {:?}", node))
    }

    fn must_computed_slot(&self, node: Node) -> Ptr<MultiSlot> {
        self.must_computed(node).must_value()
    }

    fn mark_computed(&mut self, node: Node, computed: Computed) {
        let did_overwrite = self.computed.insert(node, computed);
        debug_assert!(did_overwrite.is_none(), "duplicate computed for {:?}", node);
    }

    fn gen_value(
        &mut self,
        graph: &BlockGraph,
        block: Ptr<BasicBlock>,
        alloc: &Allocator,
        out_value: Node,
    ) {
        out_value.walk_dfs_in_block(block.firm, &mut |n| {
            self.gen_value_walk_callback(graph, block, alloc, n)
        });
    }

    fn gen_dst_slot(
        &mut self,
        block: Ptr<BasicBlock>,
        node: Node,
        alloc: &Allocator,
    ) -> Ptr<MultiSlot> {
        let dst_slot = block.new_private_slot(node, alloc);
        self.mark_computed(node, Computed::Value(dst_slot));
        dst_slot
    }

    fn gen_operand_jit(&self, node: Node) -> Operand {
        match node {
            Node::Const(c) => Operand::Imm(c.tarval()),
            Node::Proj(_, ProjKind::Start_TArgs_Arg(idx, ..)) => Operand::Param { idx },
            n => Operand::Slot(self.must_computed_slot(n)),
        }
    }

    fn gen_value_walk_callback(
        &mut self,
        graph: &BlockGraph,
        block: Ptr<BasicBlock>,
        alloc: &Allocator,
        node: Node,
    ) {
        log::debug!("visit node={:?}", node);
        if self.is_computed(node) {
            log::debug!("\tnode is already computed");
            return;
        }

        use self::{BinopKind::*, UnopKind::*};
        use libfirm_rs::Mode;
        macro_rules! binop_operand {
            ($side:ident, $op:expr) => {{
                let side = $op.$side();
                self.gen_operand_jit(side)
            }};
        }
        macro_rules! gen_binop_with_dst {
            ($kind:ident, $op:expr, $block:expr, $node:expr) => {{
                let (src1, src2, dst) = gen_binop_with_dst!(@INTERNAL, $op, $block, $node);
                self.code.body.push(Instruction::Binop {
                    kind: $kind,
                    src1,
                    src2,
                    dst,
                });
            }};
            (@DIV, $op:expr, $block:expr, $node:expr) => {{
                let (src1, src2, dst) = gen_binop_with_dst!(@INTERNAL, $op, $block, $node);
                self.code.body.push(Instruction::Div {
                    src1,
                    src2,
                    dst,
                });
            }};
            (@MOD, $op:expr, $block:expr, $node:expr) => {{
                let (src1, src2, dst) = gen_binop_with_dst!(@INTERNAL, $op, $block, $node);
                self.code.body.push(Instruction::Mod {
                    src1,
                    src2,
                    dst,
                });
            }};
            (@INTERNAL, $op:expr, $block:expr, $node:expr) => {{
                let src1 = binop_operand!(left, $op);
                let src2 = binop_operand!(right, $op);
                let dst = self.gen_dst_slot($block, $node, alloc);
                (src1, src2, dst)
            }};
        }
        match node {
            Node::Start(_)
            | Node::Const(_)
            | Node::Proj(_, ProjKind::Start_TArgs_Arg(..))
            | Node::Address(_) => (), // ignored, computed JIT

            Node::Add(add) => gen_binop_with_dst!(Add, add, block, node),
            Node::Sub(sub) => gen_binop_with_dst!(Sub, sub, block, node),
            Node::Mul(mul) => gen_binop_with_dst!(Mul, mul, block, node),
            Node::Div(div) => gen_binop_with_dst!(@DIV, div, block, node),
            Node::Mod(mod_) => gen_binop_with_dst!(@MOD, mod_, block, node),
            Node::And(and) => gen_binop_with_dst!(And, and, block, node),
            Node::Or(or) => gen_binop_with_dst!(Or, or, block, node),
            Node::Not(_) | Node::Minus(_) => {
                let dst = self.gen_dst_slot(block, node, alloc);
                let kind = if let Node::Not(_) = node { Not } else { Neg };
                self.code.body.push(Instruction::Unop {
                    kind,
                    op: Operand::Slot(dst),
                });
            }
            Node::Return(ret) => {
                let value: Option<Operand> = if ret.return_res().len() != 0 {
                    assert_eq!(ret.return_res().len(), 1);
                    log::debug!("{:?}", ret);
                    Some(self.gen_operand_jit(ret.return_res().idx(0).unwrap()))
                } else {
                    None
                };
                let end_block = graph.end_block;
                self.code.leave.push(Leave::Return { value, end_block });
            }
            Node::Jmp(jmp) => {
                let firm_target_block = jmp.out_target_block().unwrap();
                let target = block.graph.get_block(firm_target_block);
                self.code.leave.push(Leave::Jmp { target });
            }
            Node::Proj(proj, _kind) => {
                let pred = proj.pred();
                if !self.is_computed(pred) {
                    debug_assert!(
                        Node::is_address(pred)
                            || Node::is_start(pred)
                            || pred.mode() == Mode::M()
                            || pred.mode() == Mode::X(),
                        "predecessor must produce value"
                    );
                } else {
                    // the normal case
                    // copy the value produced by predecessor
                    let pred_slot = self.must_computed(pred).clone();
                    self.mark_computed(node, pred_slot);
                }
            }

            // Cmp and Cond are handled together
            Node::Cmp(cmp) => {
                let succs = cmp.out_nodes().collect::<Vec<_>>();
                assert_eq!(1, succs.len());
                match succs[0] {
                    Node::Cond(_) => (),
                    x => panic!(
                        "{:?} expected to be followed by Cond node, but got {:?}",
                        cmp, x
                    ),
                }
            }
            Node::Cond(cond) => {
                let preds = cond.in_nodes().collect::<Vec<_>>();
                assert_eq!(1, preds.len());
                let cmp = match preds[0] {
                    Node::Cmp(cmp) => cmp,
                    x => panic!(
                        "{:?} expected to be preceded by Cmp node, but got {:?}",
                        cond, x
                    ),
                };
                let op: CondOp = cmp.relation().try_into().unwrap();
                let lhs = self.gen_operand_jit(cmp.left());
                let rhs = self.gen_operand_jit(cmp.right());
                macro_rules! cond_target {
                    ($branch:expr) => {{
                        let (_, block) = cond.out_proj_target_block($branch).unwrap();
                        graph.get_block(block)
                    }};
                }
                let true_target = cond_target!(true);
                let false_target = cond_target!(false);
                self.code.leave.push(Leave::CondJmp {
                    op,
                    lhs,
                    rhs,
                    true_target,
                    false_target,
                });
            }

            Node::Call(call) => {
                log::debug!("call={:?}", call);
                let func: Entity = match call.ptr() {
                    Node::Address(addr) => addr.entity(),
                    x => panic!("call must go to an Address node, got {:?}", x),
                };
                assert!(func.ty().is_method());
                log::debug!("called func={:?}", func.ld_name());
                // extract the result (a tuple)
                // and extract that result tuple's 0th component if the function
                // returns

                let args = call
                    .args()
                    .map(|node| {
                        log::debug!("\tparam node {:?}", node);
                        self.gen_operand_jit(node)
                    })
                    .collect();

                // TODO use type_system data here? need some cross reference...
                let n_res = if let Ty::Method(ty) = func.ty() {
                    ty.res_count()
                } else {
                    panic!("The type of a function is a method type");
                };
                let dst = if n_res == 0 {
                    self.mark_computed(node, Computed::Void);
                    None
                } else if n_res == 1 {
                    Some(self.gen_dst_slot(block, node, alloc))
                } else {
                    panic!("functions must return 0 or 1 result {:?}", n_res);
                };

                let func_name = func.ld_name().to_str().unwrap().to_owned();
                self.code.body.push(Instruction::Call {
                    func: func_name,
                    args,
                    dst,
                });
            }
            Node::Unknown(_) => {
                // TODO ??? this happens in the runtime function wrapper's
                // arguments
                //
                // write somehting to computed for now
                //
                let dst_slot = block.new_private_slot(node, alloc);
                self.mark_computed(node, Computed::Value(dst_slot));
            }
            Node::Load(load) => {
                let src = self.gen_address_computation(load.ptr());
                let dst = self.gen_dst_slot(block, node, alloc);
                self.code.body.push(Instruction::LoadMem { src, dst });
            }
            Node::Store(store) => {
                let src = self.gen_operand_jit(store.value());
                let dst = self.gen_address_computation(store.ptr());
                self.code.body.push(Instruction::StoreMem { src, dst });
                let dst_slot = block.new_private_slot(node, alloc);
                self.mark_computed(node, Computed::Value(dst_slot));
            }
            x => self.comment(format_args!("\t\t\tunimplemented: {:?}", x)),
        }
    }

    fn gen_address_computation(&self, node: Node) -> AddressComputation<Operand> {
        match node {
            Node::Member(member) => {
                let base = self.gen_operand_jit(member.ptr());
                let index = IndexComputation::Zero;
                let offset = member.entity().offset() as isize;

                AddressComputation {
                    offset,
                    base,
                    index,
                }
            }
            Node::Sel(sel) => {
                let base = self.gen_operand_jit(sel.ptr());
                let elem_ty = if let Ty::Array(arr) = sel.ty() {
                    arr.element_type()
                } else {
                    unreachable!("Sel has always ArrayTy");
                };
                let elem_size = elem_ty.size();

                let index = if elem_size == 0 {
                    IndexComputation::Zero
                } else {
                    let idx = self.gen_operand_jit(sel.index());
                    IndexComputation::Displacement(
                        idx,
                        match elem_size {
                            1 => Stride::One,
                            2 => Stride::Two,
                            4 => Stride::Four,
                            8 => Stride::Eight,
                            _ => unreachable!("Unexpected element size: {}", elem_size),
                        },
                    )
                };

                AddressComputation {
                    offset: 0,
                    base,
                    index,
                }
            }
            _ => unreachable!("Load/Store nodes only have Sel and Member nodes as input"),
        }
    }
}
