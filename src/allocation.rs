use super::address;
use crate::sync::{Arc, RwLock};

pub type Ref = Arc<RwLock<Allocations>>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Allocation {
    pub id: usize,
    pub name: Option<String>,
    pub start_addr: address,
    pub end_addr: Option<address>,
}

impl Allocation {
    #[inline]
    pub fn relative_addr(&self, addr: u64) -> Option<super::address> {
        addr.checked_sub(self.start_addr)
    }

    pub fn contains(&self, addr: address) -> bool {
        let end_addr = self
            .end_addr
            .unwrap_or(self.start_addr)
            .max(self.start_addr);
        (self.start_addr..end_addr).contains(&addr)
    }

    pub fn num_bytes(&self) -> u64 {
        self.end_addr
            .and_then(|end_addr| end_addr.checked_sub(self.start_addr))
            .unwrap_or(0)
    }
}

impl std::cmp::Ord for Allocation {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.id.cmp(&other.id)
    }
}

impl std::cmp::PartialOrd for Allocation {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for Allocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let num_bytes = self.end_addr.map(|end| end - self.start_addr);
        let num_f32 = num_bytes.map(|num_bytes| num_bytes / 4);
        f.debug_struct("Allocation")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("start_addr", &self.start_addr)
            .field("end_addr", &self.end_addr)
            .field(
                "size",
                &num_bytes.map(|num_bytes| human_bytes::human_bytes(num_bytes as f64)),
            )
            .field("num_f32", &num_f32)
            .finish()
    }
}

#[derive(Debug)]
pub struct Allocations(RwLock<rangemap::RangeMap<address, Allocation>>);

impl std::ops::Deref for Allocations {
    type Target = RwLock<rangemap::RangeMap<address, Allocation>>;
    // type Target = rangemap::RangeMap<address, Allocation>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Default for Allocations {
    fn default() -> Self {
        Self(RwLock::new(rangemap::RangeMap::new()))
    }
}

impl Allocations {
    // pub fn iter(&self) -> rangemap::map::Iter<'_, u64, Allocation> {
    //     let lock = self.0.read();
    //     lock.iter()
    // }
    //
    // pub fn get(&self, addr: &address) -> Option<&Allocation> {
    //     let lock = self.0.read();
    //     lock.get(addr)
    // }

    pub fn insert(&self, range: std::ops::Range<address>, name: Option<String>) {
        let mut lock = self.0.write();

        // check for intersections
        if lock.overlaps(&range) {
            log::warn!("overlapping memory allocation {:?}", &range);
        }
        // assert!(
        //     !self.0.overlaps(&range),
        //     "overlapping memory allocation {:?}",
        //     &range
        // );
        let id = lock.len() + 1; // zero is reserved for instructions
        let start_addr = range.start;
        let end_addr = Some(range.end);
        lock.insert(
            range,
            Allocation {
                id,
                name,
                start_addr,
                end_addr,
            },
        );
    }
}
