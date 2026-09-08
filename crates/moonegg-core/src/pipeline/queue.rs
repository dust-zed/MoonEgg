use std::collections::VecDeque;

#[derive(Debug)]
pub struct BoundedQueue<T> {
    items: VecDeque<T>,
    capacity: usize,
}

impl<T> BoundedQueue<T> {
    pub fn new(capacity: usize) -> Result<Self, QueueError> {
        if capacity == 0 {
            return Err(QueueError::InvalidCapacity);
        }

        Ok(Self {
            items: VecDeque::with_capacity(capacity),
            capacity,
        })
    }

    pub fn push(&mut self, item: T) -> QueuePushResult<T> {
        if self.is_full() {
            return QueuePushResult::Full(item);
        }
        self.items.push_back(item);
        QueuePushResult::Accepted
    }

    pub fn drain(&mut self) -> impl Iterator<Item = T> + '_ {
        self.items.drain(..)
    }

    pub fn is_full(&self) -> bool {
        self.items.len() == self.capacity
    }

    pub fn pop(&mut self) -> Option<T> {
        self.items.pop_front()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    InvalidCapacity,
}

#[must_use = "queue push result be handled"]
#[derive(Debug)]
pub enum QueuePushResult<T> {
    Accepted,
    Full(T),
}

#[cfg(test)]
mod tests {

    use crate::pipeline::queue::{BoundedQueue, QueuePushResult};

    #[derive(Debug, PartialEq, Eq)]
    struct TestToken(String);

    fn test_bounded_queue() -> BoundedQueue<TestToken> {
        BoundedQueue::new(2).unwrap()
    }

    fn token(value: &str) -> TestToken {
        TestToken(value.to_owned())
    }

    #[test]
    fn full_queue_returns_item_and_preserves_fifo_order() {
        let mut queue = test_bounded_queue();

        assert!(matches!(queue.push(token("1")), QueuePushResult::Accepted));

        assert!(matches!(queue.push(token("2")), QueuePushResult::Accepted));

        assert!(queue.is_full());
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.capacity(), 2);

        let returned = match queue.push(token("3")) {
            QueuePushResult::Accepted => {
                panic!("expected full queue to return the item")
            }
            QueuePushResult::Full(item) => item,
        };

        assert_eq!(returned, token("3"));

        assert_eq!(queue.len(), 2);

        assert_eq!(queue.pop(), Some(token("1")));
        assert_eq!(queue.pop(), Some(token("2")));
        assert_eq!(queue.pop(), None);
        assert!(queue.is_empty());
    }

    #[test]
    fn drain_returns_all_items_and_empties_queue() {
        let mut queue = test_bounded_queue();

        assert!(matches!(queue.push(token("1")), QueuePushResult::Accepted));

        assert!(matches!(queue.push(token("2")), QueuePushResult::Accepted));

        let drained: Vec<TestToken> = queue.drain().collect();

        assert_eq!(drained, vec![token("1"), token("2")]);

        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
    }
}
