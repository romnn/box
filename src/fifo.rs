use crate::interconn as ic;
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Fifo<T> {
    inner: VecDeque<T>,
    // min_size: Option<usize>,
    max_size: Option<usize>,
}

impl<T> std::iter::IntoIterator for Fifo<T> {
    type Item = T;
    type IntoIter = std::collections::vec_deque::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}

impl<T> std::fmt::Display for Fifo<T>
where
    T: std::fmt::Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Fifo({:>2}/{:<2}){:#?}",
            self.inner.len(),
            self.max_size
                .map(|max| max.to_string())
                .as_deref()
                .unwrap_or(""),
            self.inner
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
        )
    }
}

impl<T> Fifo<T> {
    #[must_use]
    pub fn iter(&self) -> std::collections::vec_deque::Iter<T> {
        self.inner.iter()
    }
}

impl<T> Fifo<T> {
    #[must_use]
    // pub fn new(min_size: Option<usize>, max_size: Option<usize>) -> Self {
    pub fn new(max_size: Option<usize>) -> Self {
        Self {
            inner: VecDeque::new(),
            // min_size,
            max_size,
        }
    }

    // TODO: change to push_back
    pub fn enqueue(&mut self, value: T) {
        self.inner.push_back(value);
    }

    // TODO: change to pop_front
    pub fn dequeue(&mut self) -> Option<T> {
        self.inner.pop_front()
    }

    #[must_use]
    pub fn first(&self) -> Option<&T> {
        // debug_assert_eq!(self.inner.get(0), self.inner.front());
        self.inner.front()
    }

    #[must_use]
    pub fn full(&self) -> bool {
        // log::trace!(
        //     "FIFO full? max len={:?} length={}",
        //     self.max_size,
        //     self.inner.len()
        // );
        match self.max_size {
            Some(max) => self.inner.len() >= max,
            None => false,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    #[must_use]
    pub fn can_fit(&self, n: usize) -> bool {
        match self.max_size {
            Some(max) => self.inner.len() + n <= max,
            None => true,
        }
    }

    pub fn drain(&mut self) -> std::collections::vec_deque::Drain<'_, T> {
        self.inner.drain(..)
    }
}

impl<P> ic::Connection<P> for Fifo<P>
where
    P: Send + Sync + 'static,
{
    fn can_send(&self, packets: &[u32]) -> bool {
        match self.max_size {
            Some(max) => self.inner.len() + packets.len() <= max,
            None => true,
        }
    }

    fn send(&mut self, packet: P) {
        self.inner.push_back(packet);
    }

    fn receive(&mut self) -> Option<P> {
        self.inner.pop_front()
    }
}

impl<P> ic::BufferedConnection<P> for Fifo<P>
where
    P: Send + Sync + 'static,
{
    fn buffered(&self) -> Box<dyn Iterator<Item = &P> + '_> {
        Box::new(self.iter())
    }

    fn num_buffered(&self) -> usize {
        self.len()
    }

    fn drain(&mut self) -> Box<dyn Iterator<Item = P> + '_> {
        Box::new(self.inner.drain(..))
    }
}
