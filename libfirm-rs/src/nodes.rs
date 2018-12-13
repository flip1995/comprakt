use crate::{
    nodes_gen::{self, Block, Node, NodeFactory, Phi, Proj},
    tarval::Tarval,
};
use libfirm_rs_bindings as bindings;
use std::{
    ffi::CStr,
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

// FIXME generate this
impl Into<*mut bindings::ir_node> for Node {
    fn into(self) -> *mut bindings::ir_node {
        self.internal_ir_node()
    }
}

impl fmt::Debug for nodes_gen::Call {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Call to {:?} {}", self.ptr(), self.node_id())
    }
}

impl nodes_gen::Address {
    pub fn entity(self) -> *mut bindings::ir_entity {
        unsafe { bindings::get_Address_entity(self.internal_ir_node()) }
    }

    pub fn set_entity(self, ir_entity: *mut bindings::ir_entity) {
        unsafe {
            bindings::set_Address_entity(self.internal_ir_node(), ir_entity);
        }
    }
}

impl nodes_gen::Block {
    pub fn num_cfgpreds(self) -> i32 {
        unsafe { bindings::get_Block_n_cfgpreds(self.internal_ir_node()) }
    }
}

impl fmt::Debug for nodes_gen::Address {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let entity = self.entity();
        let entity_name = unsafe { CStr::from_ptr(bindings::get_entity_name(entity)) };
        write!(f, "Address of {:?} {}", entity_name, self.node_id())
    }
}

pub trait BinOp {
    fn left(&self) -> Node;
    fn right(&self) -> Node;
    fn compute(&self, left: Tarval, right: Tarval) -> Tarval;
}

macro_rules! binop_impl {
    ($node_ty: ident, $compute: expr) => {
        impl BinOp for $node_ty {
            fn left(&self) -> Node {
                $node_ty::left(*self)
            }
            fn right(&self) -> Node {
                $node_ty::right(*self)
            }
            fn compute(&self, left: Tarval, right: Tarval) -> Tarval {
                $compute(self, left, right)
            }
        }
    };
}

use self::nodes_gen::{Add, Cmp, Div, Eor, Mod, Mul, Sub};

binop_impl!(Add, |_n, l, r| l + r);
binop_impl!(Sub, |_n, l, r| l - r);
binop_impl!(Mul, |_n, l, r| l * r);
binop_impl!(Div, |_n, l, r| l / r);
binop_impl!(Mod, |_n, l, r| l % r);
binop_impl!(Eor, |_n, l, r| l ^ r);
binop_impl!(Cmp, |n: &Cmp, l: Tarval, r| l.lattice_cmp(n.relation(), r));

macro_rules! try_as_bin_op {
    ($($node_ty: ident),*) => (
        pub fn try_as_bin_op(node: &Node) -> Result<&dyn BinOp, ()> {
            match node {
                $(
                    Node::$node_ty(node) => Ok(node),
                )*
                _ => Err(()),
            }
        }
    );
}

try_as_bin_op!(Add, Sub, Mul, Div, Mod, Eor, Cmp);

pub trait UnaryOp {
    fn operand(&self) -> Node;
    fn compute(&self, val: Tarval) -> Tarval;
}

impl UnaryOp for nodes_gen::Conv {
    fn operand(&self) -> Node {
        self.op()
    }
    fn compute(&self, val: Tarval) -> Tarval {
        val.cast(self.mode()).unwrap_or_else(Tarval::bad)
    }
}

impl UnaryOp for nodes_gen::Minus {
    fn operand(&self) -> Node {
        self.op()
    }
    fn compute(&self, val: Tarval) -> Tarval {
        -val
    }
}

pub fn try_as_unary_op(node: &Node) -> Result<&dyn UnaryOp, ()> {
    match node {
        Node::Minus(node) => Ok(node),
        Node::Conv(node) => Ok(node),
        _ => Err(()),
    }
}
