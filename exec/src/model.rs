pub use trace_model::Dim;

#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemorySpace {
    Local,
    Shared,
    Constant,
    Texture,
    Global,
}

impl From<MemorySpace> for trace_model::MemorySpace {
    fn from(space: MemorySpace) -> Self {
        match space {
            MemorySpace::Local => Self::Local,
            MemorySpace::Shared => Self::Shared,
            MemorySpace::Constant => Self::Constant,
            MemorySpace::Texture => Self::Texture,
            MemorySpace::Global => Self::Global,
        }
    }
}

impl MemorySpace {
    pub fn base_addr(self) -> u64 {
        trace_model::MemorySpace::from(self).base_addr()
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub enum MemAccessKind {
    Load,
    Store,
}

/// Memory instruction
#[derive(Debug, Clone, Hash, Ord, PartialOrd)]
pub struct MemInstruction {
    pub mem_space: MemorySpace,
    pub kind: MemAccessKind,
    pub bypass_l1: bool,
    pub bypass_l2: bool,
    pub addr: u64,
    pub size: u32,
}

impl std::fmt::Display for MemInstruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}[{:?}]@{}", self.kind, self.mem_space, self.addr)
    }
}

impl Eq for MemInstruction {}

impl PartialEq for MemInstruction {
    fn eq(&self, other: &Self) -> bool {
        (self.mem_space, self.kind).eq(&(other.mem_space, other.kind))
    }
}

/// Instruction
#[derive(Debug, Clone, Hash, PartialEq, Eq, Ord, PartialOrd)]
pub enum Instruction {
    Memory(MemInstruction),
    Barrier,
}

/// Thread instruction
#[derive(Debug, Clone, PartialOrd, Ord)]
pub enum ThreadInstruction {
    Access(MemInstruction),
    Barrier,
    Nop,
    Branch(usize),
    TookBranch(usize),
    Reconverge(usize),
}

impl From<MemInstruction> for ThreadInstruction {
    fn from(inst: MemInstruction) -> Self {
        Self::Access(inst)
    }
}

impl ThreadInstruction {
    #[must_use]
    pub fn is_access(&self) -> bool {
        matches!(self, ThreadInstruction::Access(_))
    }

    #[must_use]
    pub fn instruction(&self) -> Option<&MemInstruction> {
        match self {
            ThreadInstruction::Access(access) => Some(access),
            _ => None,
        }
    }
}

impl std::fmt::Display for ThreadInstruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Access(inst) => write!(f, "Access({inst})"),
            other => std::fmt::Debug::fmt(other, f),
        }
    }
}

impl Eq for ThreadInstruction {}

#[allow(clippy::match_same_arms)]
impl PartialEq for ThreadInstruction {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ThreadInstruction::Nop, _) => true,
            (_, ThreadInstruction::Nop) => true,
            (ThreadInstruction::Access(a), ThreadInstruction::Access(b)) => a.eq(b),
            (ThreadInstruction::Branch(a), ThreadInstruction::Branch(b)) => a.eq(b),
            (ThreadInstruction::TookBranch(a), ThreadInstruction::TookBranch(b)) => a.eq(b),
            (ThreadInstruction::Reconverge(a), ThreadInstruction::Reconverge(b)) => a.eq(b),
            (_, _) => false,
        }
    }
}
