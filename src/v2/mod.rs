use std::cmp::max;
use std::convert::TryFrom;
use std::ops::{Shl, Shr};

pub mod bench_fun;

/*
   Design philosophy: There is the `Bdd` object representing a stand-alone `Bdd`, and then there
   is a `BddPool` which stores (possibly) multiple `Bdds` in one buffer.

   For manipulating the standalone objects, there are some "optimization tricks", but we generally
   don't do anything too magical, like pointer compression or thread local storage for buffer
   caching. Either the `Bdd` is small and everything can be allocated on the stack, or the `Bdd`
   is large and everything is constructed and destroyed for each operation separately,
   resulting in a new `Bdd`.

   For `BddPool`, the situation is a bit different. In particular, there is a shared task and
   node cache. Task cache has to be cleared between each operation, but the node cache can be
   inherited entirely. There is also another trick: If the amount of nodes is small, we use
   compressed pointers (u16 or u32 instead of u64) which saves a bit of memory, but more
   importantly, it saves a lot of wasted cache bandwidth.

   The compression works like this:
    - If (before operation), the pool is using more than 1/2 of its *current* address space, it is
    expanded into larger pointers. Similarly, if it is using less then 1/4 of the *target*
    address space, it is contracted.
    - When performing an operation, we regularly check for an overflow. If overflow happens,
    the operation "raises and exception" and triggers expansion, after which the operation is
    restarted.

    From time to time, we have to run garbage collection to make sure old nodes are discarded.

    On `BddPool`, there are two types of operations:
       - Internal, meaning two `Bdds` in the same pool are manipulated.
       - External, meaning an internal `Bdd` and a stand-alone `Bdd` object are considered.
*/

pub mod _impl_;

/// **(internal)** A pointer into the `Bdd` graph. Its actual range is 6 bytes, so `0..(2^48 - 1)`.
/// This allows indexing graphs which are ~4TB each. That should be enough for the foreseeable
/// future and a bit more.
///
/// We assume it can always be safely converted to usize. When a `BddPool` with pointer compression
/// is used, the pointer may be safe to convert also to lower types (`u32` or `u16`), but this
/// very much depends on context, so be careful.
///
/// We *may* check some of the conversions at runtime, but in general this is an *unsafe* land.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct NodeId(u64);

/// Index of a `Bdd` variable. It's range is `0..(2^16 - 1)`, but the last value is reserved
/// as an *undefined* value.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct VariableId(u16);

impl From<u16> for VariableId {
    fn from(value: u16) -> Self {
        VariableId(value)
    }
}

/// A single stand-alone binary decision diagram.
///
/// A `Bdd` is immutable and operations on `Bdds` always create new `Bdds`. This makes it well
/// suited for parallel processing or sharing between threads. If you want a mutable `Bdd`,
/// look at the `BddPool`, which allows multiple diagrams in one storage pool and reuses memory
/// of existing diagrams.
///
/// A `Bdd` object is not guaranteed to be minimal or canonical. In general we try to create `Bdds`
/// which are as small as possible, but we prefer speed to minimality.
pub struct Bdd {
    variable_count: u16,
    nodes: Vec<BddNode>,
}

impl Bdd {

    /*
    /// This is not ideal, but it allows to calls like
    /// a.and(b), assuming b is not used anywhere else anymore.
    /// And if you need to use B, then you can still use a.and(&b).
    pub fn and<B: AsRef<Bdd>>(&self, other: B) -> Bdd {
        todo!()
    }*/

}

/// A `BddNode` packs together the decision variable and two pointers: low/high. It is slightly
/// more memory efficient than just storing the values directly.
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct BddNode(u64, u64);

/// A collection of binary decision diagrams.
pub struct BddPool {}

// TODO: Move this to separate modules:

impl BddNode {
    // A mask with bits set where variable bits are stored in a u64.
    const VARIABLE_MASK: u64 = (u16::MAX as u64) << 48;
    // A mask with bits set where id bits are stored in a u64.
    const ID_MASK: u64 = !Self::VARIABLE_MASK;
    pub const ZERO: BddNode = BddNode(Self::VARIABLE_MASK, 0);
    pub const ONE: BddNode = BddNode(Self::VARIABLE_MASK | 1, 1);

    #[inline]
    pub(crate) fn unpack(self) -> (VariableId, NodeId, NodeId) {
        let (x, y) = (self.0, self.1);
        (
            VariableId(x.shr(48) as u16),
            NodeId(x & Self::ID_MASK),
            NodeId(y),
        )
    }

    #[inline]
    pub fn high_link(&self) -> NodeId {
        NodeId(self.1)
    }

    #[inline]
    pub fn low_link(&self) -> NodeId {
        NodeId(self.0 & Self::ID_MASK)
    }

    #[inline]
    pub(crate) fn pack(variable: VariableId, low: NodeId, high: NodeId) -> BddNode {
        let x = u64::from(variable.0).shl(48) | low.0;
        BddNode(x, high.0)
    }
}

impl NodeId {
    pub const ZERO: NodeId = NodeId(0);
    pub const ONE: NodeId = NodeId(1);
    pub const UNDEFINED: NodeId = NodeId(u64::MAX);

    #[inline]
    pub fn is_undefined(&self) -> bool {
        self.0 == u64::MAX
    }

    #[inline]
    pub fn is_zero(&self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub fn is_one(&self) -> bool {
        self.0 == 1
    }

    /// Unsafe conversion to usize. Generally ok, but may fail on 32-bit platforms.
    #[inline]
    pub unsafe fn as_index_unchecked(self) -> usize {
        self.0 as usize
    }

    /// A safe conversion to usize.
    ///
    /// Don't use this in performance critical code unless you really really have to.
    pub fn as_index(self) -> usize {
        usize::try_from(u64::from(self)).unwrap()
    }
}

impl VariableId {
    pub const UNDEFINED: VariableId = VariableId(u16::MAX);
}

impl From<NodeId> for u64 {
    fn from(value: NodeId) -> Self {
        value.0
    }
}

impl Bdd {
    #[inline]
    pub fn variable_count(&self) -> u16 {
        self.variable_count
    }

    #[inline]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    #[inline]
    pub(crate) fn root_node(&self) -> NodeId {
        // Conversion is safe because the max. number of nodes is 2^48 - 1
        NodeId((self.nodes.len() - 1) as u64)
    }

    pub fn new_false() -> Bdd {
        Bdd {
            variable_count: 0,
            nodes: vec![BddNode::ZERO],
        }
    }

    pub(crate) fn true_with_capacity(capacity: usize) -> Bdd {
        let mut bdd = Bdd {
            variable_count: 0,
            nodes: Vec::with_capacity(capacity),
        };
        bdd.nodes.push(BddNode::ZERO);
        bdd.nodes.push(BddNode::ONE);
        bdd
    }

    pub fn new_variable(variable: VariableId) -> Bdd {
        Bdd {
            variable_count: variable.0 + 1,
            nodes: vec![
                BddNode::ZERO,
                BddNode::ONE,
                BddNode::pack(variable, NodeId::ZERO, NodeId::ONE),
            ],
        }
    }

    pub fn update_variable_count(&mut self, variables: u16) {
        self.variable_count = max(self.variable_count, variables);
    }

    #[inline]
    pub(crate) fn push_node(&mut self, node: BddNode) -> NodeId {
        self.nodes.push(node);
        self.root_node()
    }

    pub(crate) fn get_node(&self, id: NodeId) -> BddNode {
        self.nodes[id.as_index()]
    }

    #[inline]
    pub(crate) unsafe fn get_node_unchecked(&self, id: NodeId) -> BddNode {
        debug_assert!(id.as_index() < self.nodes.len());
        unsafe { *self.nodes.get_unchecked(id.as_index_unchecked()) }
    }

    #[inline]
    pub(crate) fn prefetch(&self, id: NodeId) {
        unsafe {
            // Prefetch operations ignore memory errors and are therefore "externally safe".
            if cfg!(target_arch = "x86_64") {
                let reference: *const BddNode = self.nodes.get_unchecked(id.0 as usize);
                std::arch::x86_64::_mm_prefetch::<3>(reference as *const i8);
            }
        }
    }

    pub(crate) fn get_variable(&self, id: NodeId) -> VariableId {
        debug_assert!((id.0 as usize) < self.nodes.len());
        let node = unsafe { self.nodes.get_unchecked(id.0 as usize) };
        VariableId(node.0.shr(48) as u16)
    }

    pub fn sort_preorder_safe(&mut self) {
        if self.nodes.len() < 2 {
            return;
        }

        let mut new_id = vec![0usize; self.nodes.len()];
        new_id[0] = 0;
        new_id[1] = 1;

        let mut stack = Vec::new();
        stack.push(self.root_node());

        let mut new_index = self.nodes.len() - 1;
        while let Some(top) = stack.pop() {
            if top.is_zero() || top.is_one() {
                continue;
            }

            let current_index = unsafe { top.as_index_unchecked() };
            if new_id[current_index] == 0 {
                new_id[current_index] = new_index;
                new_index -= 1;

                let node = unsafe { self.get_node_unchecked(top) };
                stack.push(node.high_link());
                stack.push(node.low_link());
            }
        }

        assert_eq!(new_index, 1);

        let mut new_nodes =
            vec![BddNode::pack(VariableId(0), NodeId(0), NodeId(0)); self.node_count()];

        for old_index in 0..self.node_count() {
            let (variable, low_link, high_link) =
                unsafe { self.get_node_unchecked(NodeId(old_index as u64)) }.unpack();
            let new_index = new_id[old_index];
            let new_low = new_id[unsafe { low_link.as_index_unchecked() }];
            let new_high = new_id[unsafe { high_link.as_index_unchecked() }];

            new_nodes[new_index] =
                BddNode::pack(variable, NodeId(new_low as u64), NodeId(new_high as u64));
        }

        self.nodes = new_nodes;
    }

    pub fn sort_preorder(&mut self) {
        if self.nodes.len() < 2 {
            return;
        }
        // Bdd sorted in pre-order is faster to iterate due to cache locality.
        let mut new_id = vec![0usize; self.nodes.len()];
        new_id[0] = 0;
        new_id[1] = 1;

        let mut stack_index_after_last: usize = 0;
        let mut stack = vec![NodeId::ZERO; 3 * usize::from(self.variable_count())];
        unsafe {
            *stack.get_unchecked_mut(stack_index_after_last) = self.root_node();
            stack_index_after_last += 1;
        }

        let mut new_index = self.nodes.len() - 1;
        while stack_index_after_last > 0 {
            let top = unsafe { *stack.get_unchecked(stack_index_after_last - 1) };
            stack_index_after_last -= 1;

            if top.is_one() || top.is_zero() {
                continue;
            }

            let index = unsafe { top.as_index_unchecked() };
            let new_id_cell = unsafe { new_id.get_unchecked_mut(index) };
            if *new_id_cell == 0 {
                *new_id_cell = new_index;
                new_index -= 1;
                let (_, low, high) = unsafe { self.get_node_unchecked(top) }.unpack();
                unsafe {
                    *stack.get_unchecked_mut(stack_index_after_last) = high;
                    *stack.get_unchecked_mut(stack_index_after_last + 1) = low;
                    stack_index_after_last += 2;
                }
            }
        }

        let mut new_nodes = Bdd::true_with_capacity(self.node_count()).nodes;
        // Allocate nodes without initialization
        unsafe { new_nodes.set_len(self.node_count()) };
        for old_index in 2..new_id.len() {
            let (var, old_low, old_high) = unsafe { self.nodes.get_unchecked(old_index) }.unpack();

            let new_index = unsafe { *new_id.get_unchecked(old_index) };
            let new_low = unsafe { *new_id.get_unchecked(old_low.as_index_unchecked()) };
            let new_high = unsafe { *new_id.get_unchecked(old_high.as_index_unchecked()) };
            unsafe {
                let cell = new_nodes.get_unchecked_mut(new_index);
                *cell = BddNode::pack(var, NodeId(new_low as u64), NodeId(new_high as u64));
            }
        }

        self.nodes = new_nodes;
    }
}

impl TryFrom<&str> for Bdd {
    type Error = String;

    fn try_from(data: &str) -> Result<Self, Self::Error> {
        //let mut node_variables = Vec::new();
        //let mut node_pointers = Vec::new();
        let mut nodes = Vec::new();
        for node_string in data.split('|').filter(|s| !s.is_empty()) {
            let mut node_items = node_string.split(',');
            let variable = node_items.next();
            let left_pointer = node_items.next();
            let right_pointer = node_items.next();
            if node_items.next().is_some()
                || variable.is_none()
                || left_pointer.is_none()
                || right_pointer.is_none()
            {
                return Err(format!("Unexpected node representation `{}`.", node_string));
            }
            let variable = if let Ok(x) = variable.unwrap().parse::<u16>() {
                x
            } else {
                return Err(format!("Invalid variable numeral `{}`.", variable.unwrap()));
            };
            let left_pointer = if let Ok(x) = left_pointer.unwrap().parse::<u64>() {
                x
            } else {
                return Err(format!(
                    "Invalid pointer numeral `{}`.",
                    left_pointer.unwrap()
                ));
            };
            let right_pointer = if let Ok(x) = right_pointer.unwrap().parse::<u64>() {
                x
            } else {
                return Err(format!(
                    "Invalid pointer numeral `{}`.",
                    right_pointer.unwrap()
                ));
            };
            //node_variables.push(Variable(variable));
            //node_pointers.push(Pointer(left_pointer) | Pointer(right_pointer));
            nodes.push(BddNode::pack(
                VariableId(variable),
                NodeId(left_pointer),
                NodeId(right_pointer),
            ));
        }
        Ok(Bdd {
            variable_count: nodes[0].unpack().0 .0,
            nodes,
        })
    }
}
