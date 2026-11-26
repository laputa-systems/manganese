use crate::cmp::Cmp;
use crate::types::{Direction, LdbIterator};

use bytes::Bytes;
use std::cmp::Ordering;
use std::rc::Rc;

pub struct MergingIter {
    iters: Vec<Box<dyn LdbIterator>>,
    current: Option<usize>,
    direction: Direction,
    cmp: Rc<Box<dyn Cmp>>,
}

impl MergingIter {
    /// Construct a new merging iterator.
    pub fn new(cmp: Rc<Box<dyn Cmp>>, iters: Vec<Box<dyn LdbIterator>>) -> MergingIter {
        MergingIter {
            iters,
            current: None,
            direction: Direction::Forward,
            cmp,
        }
    }

    fn key(&self) -> Option<Bytes> {
        self.current
            .and_then(|current| self.iters[current].current())
            .map(|(key, _)| key)
    }

    fn find_smallest(&mut self) {
        let mut smallest: Option<(usize, Bytes)> = None;
        for (i, iter) in self.iters.iter().enumerate() {
            if let Some((key, _)) = iter.current() {
                let better = match &smallest {
                    None => true,
                    Some((_, best)) => self.cmp.cmp(&key, best) == Ordering::Less,
                };
                if better {
                    smallest = Some((i, key));
                }
            }
        }
        self.current = smallest.map(|(i, _)| i);
    }

    fn find_largest(&mut self) {
        let mut largest: Option<(usize, Bytes)> = None;
        for (i, iter) in self.iters.iter().enumerate().rev() {
            if let Some((key, _)) = iter.current() {
                let better = match &largest {
                    None => true,
                    Some((_, best)) => self.cmp.cmp(&key, best) == Ordering::Greater,
                };
                if better {
                    largest = Some((i, key));
                }
            }
        }
        self.current = largest.map(|(i, _)| i);
    }
}

impl LdbIterator for MergingIter {
    fn seek_to_first(&mut self) {
        for iter in self.iters.iter_mut() {
            iter.seek_to_first();
        }
        self.find_smallest();
        self.direction = Direction::Forward;
    }
    fn seek_to_last(&mut self) {
        for iter in self.iters.iter_mut() {
            iter.seek_to_last();
        }
        self.find_largest();
        self.direction = Direction::Reverse;
    }
    fn status(&self) -> crate::Result<()> {
        for iter in &self.iters {
            iter.status()?;
        }
        Ok(())
    }
    fn advance(&mut self) -> bool {
        let Some(key) = self.key() else {
            self.seek_to_first();
            return self.valid();
        };
        let current = self.current.expect("a merging iterator with a key has a current child");
        if self.direction != Direction::Forward {
            for i in 0..self.iters.len() {
                if i != current {
                    let child = &mut self.iters[i];
                    child.seek(&key);
                    if let Some((child_key, _)) = child.current() {
                        if self.cmp.cmp(&key, &child_key) == Ordering::Equal {
                            child.advance();
                        }
                    }
                }
            }
            self.direction = Direction::Forward;
        }
        self.iters[current].advance();
        self.find_smallest();
        self.valid()
    }
    fn valid(&self) -> bool {
        self.current.is_some_and(|current| self.iters[current].valid())
    }
    fn seek(&mut self, key: &[u8]) {
        for iter in self.iters.iter_mut() {
            iter.seek(key);
        }
        self.find_smallest();
        self.direction = Direction::Forward;
    }
    fn reset(&mut self) {
        for iter in self.iters.iter_mut() {
            iter.reset();
        }
        self.current = None;
        self.direction = Direction::Forward;
    }
    fn current(&self) -> Option<(Bytes, Bytes)> {
        self.current.and_then(|current| self.iters[current].current())
    }
    fn prev(&mut self) -> bool {
        let Some(key) = self.key() else {
            return false;
        };
        let current = self.current.expect("a merging iterator with a key has a current child");
        if self.direction != Direction::Reverse {
            for i in 0..self.iters.len() {
                if i != current {
                    let child = &mut self.iters[i];
                    child.seek(&key);
                    if child.valid() {
                        child.prev();
                    } else {
                        child.seek_to_last();
                    }
                }
            }
            self.direction = Direction::Reverse;
        }
        self.iters[current].prev();
        self.find_largest();
        self.valid()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::cmp::DefaultCmp;
    use crate::skipmap::tests;
    use crate::test_util::{test_iterator_properties, LdbIteratorIter, TestLdbIter};
    use crate::types::{current_key_val, LdbIterator};

    #[test]
    fn test_merging_one() {
        let skm = tests::make_skipmap();
        let iter = skm.iter();
        let mut iter2 = skm.iter();

        let mut miter = MergingIter::new(Rc::new(Box::new(DefaultCmp)), vec![Box::new(iter)]);

        while let Some((k, v)) = miter.next() {
            if let Some((k2, v2)) = iter2.next() {
                assert_eq!(k, k2);
                assert_eq!(v, v2);
            } else {
                panic!("Expected element from iter2");
            }
        }
    }

    #[test]
    fn test_merging_two() {
        let skm = tests::make_skipmap();
        let iter = skm.iter();
        let iter2 = skm.iter();

        let mut miter = MergingIter::new(
            Rc::new(Box::new(DefaultCmp)),
            vec![Box::new(iter), Box::new(iter2)],
        );

        while let Some((k, v)) = miter.next() {
            if let Some((k2, v2)) = miter.next() {
                assert_eq!(k, k2);
                assert_eq!(v, v2);
            } else {
                panic!("Odd number of elements");
            }
        }
    }

    #[test]
    fn test_merging_zero() {
        let mut miter = MergingIter::new(Rc::new(Box::new(DefaultCmp)), vec![]);
        assert_eq!(0, LdbIteratorIter::wrap(&mut miter).count());
    }

    #[test]
    fn test_merging_behavior() {
        let val = b"def";
        let iter = TestLdbIter::new(vec![(b("aba"), val), (b("abc"), val)]);
        let iter2 = TestLdbIter::new(vec![(b("abb"), val), (b("abd"), val)]);
        let miter = MergingIter::new(
            Rc::new(Box::new(DefaultCmp)),
            vec![Box::new(iter), Box::new(iter2)],
        );
        test_iterator_properties(miter);
    }

    #[test]
    fn test_merging_forward_backward() {
        let val = b"def";
        let iter = TestLdbIter::new(vec![(b("aba"), val), (b("abc"), val), (b("abe"), val)]);
        let iter2 = TestLdbIter::new(vec![(b("abb"), val), (b("abd"), val)]);

        let mut miter = MergingIter::new(
            Rc::new(Box::new(DefaultCmp)),
            vec![Box::new(iter), Box::new(iter2)],
        );

        // miter should return the following sequence: [aba, abb, abc, abd, abe]

        // -> aba
        let first = miter.next();
        // -> abb
        let second = miter.next();
        // -> abc
        let third = miter.next();
        eprintln!("{:?} {:?} {:?}", first, second, third);

        assert!(first != third);
        // abb <-
        assert!(miter.prev());
        assert_eq!(second, current_key_val(&miter));
        // aba <-
        assert!(miter.prev());
        assert_eq!(first, current_key_val(&miter));
        // -> abb
        assert!(miter.advance());
        assert_eq!(second, current_key_val(&miter));
        // -> abc
        assert!(miter.advance());
        assert_eq!(third, current_key_val(&miter));
        // -> abd
        assert!(miter.advance());
        assert_eq!(
            Some((b("abd").to_vec(), val.to_vec())),
            current_key_val(&miter)
        );
    }

    fn b(s: &'static str) -> &'static [u8] {
        s.as_bytes()
    }

    #[test]
    fn test_merging_real() {
        let val = b"def";

        let it1 = TestLdbIter::new(vec![(b("aba"), val), (b("abc"), val), (b("abe"), val)]);
        let it2 = TestLdbIter::new(vec![(b("abb"), val), (b("abd"), val)]);
        let expected = [b("aba"), b("abb"), b("abc"), b("abd"), b("abe")];

        let mut iter = MergingIter::new(
            Rc::new(Box::new(DefaultCmp)),
            vec![Box::new(it1), Box::new(it2)],
        );

        for (i, (k, _)) in LdbIteratorIter::wrap(&mut iter).enumerate() {
            assert_eq!(k, expected[i]);
        }
    }

    #[test]
    fn test_merging_seek_reset() {
        let val = b"def";

        let it1 = TestLdbIter::new(vec![(b("aba"), val), (b("abc"), val), (b("abe"), val)]);
        let it2 = TestLdbIter::new(vec![(b("abb"), val), (b("abd"), val)]);

        let mut iter = MergingIter::new(
            Rc::new(Box::new(DefaultCmp)),
            vec![Box::new(it1), Box::new(it2)],
        );

        assert!(!iter.valid());
        iter.advance();
        assert!(iter.valid());
        assert!(current_key_val(&iter).is_some());

        iter.seek(b"abc");
        assert_eq!(
            current_key_val(&iter),
            Some((b("abc").to_vec(), val.to_vec()))
        );
        iter.seek(b"ab0");
        assert_eq!(
            current_key_val(&iter),
            Some((b("aba").to_vec(), val.to_vec()))
        );
        iter.seek(b"abx");
        assert_eq!(current_key_val(&iter), None);

        iter.reset();
        assert!(!iter.valid());
        iter.next();
        assert_eq!(
            current_key_val(&iter),
            Some((b("aba").to_vec(), val.to_vec()))
        );
    }
}
