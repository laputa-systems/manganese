use std::rc::Rc;

const MIN_ALLOC_SIZE: usize = 4096;
const BUFFER_HEAD_SIZE: usize = 32;
const BUFFER_BLOCK_SIZE: usize = 24;

#[derive(Clone)]
pub struct Snapshot {
    pub bytes: Rc<Vec<u8>>,
    pub size: usize,
    spans: Rc<Vec<(usize, usize)>>,
}

impl Snapshot {
    pub fn some_data(&self, position: usize) -> Option<(usize, usize)> {
        if position >= self.size {
            return None;
        }
        self.spans.iter().find(|&&(start, end)| start <= position && position < end).map(|&(_, end)| (position, end.min(self.size)))
    }
}

#[derive(Default)]
pub struct SharedBuffer {
    bytes: Rc<Vec<u8>>,
    segments: Vec<(usize, usize)>,
}

impl SharedBuffer {
    pub fn append(&mut self, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }
        let start = self.bytes.len();
        Rc::make_mut(&mut self.bytes).extend_from_slice(chunk);
        self.segments.push((start, start + chunk.len()));
    }

    pub fn size(&self) -> usize {
        self.bytes.len()
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot { bytes: self.bytes.clone(), size: self.bytes.len(), spans: Rc::new(self.segments.clone()) }
    }

    pub fn segments_from(&self, position: usize) -> Vec<(usize, usize)> {
        self.segments.iter().filter(|&&(_, end)| end > position).map(|&(start, end)| (start.max(position), end)).collect()
    }
}

pub struct RwBuffer {
    blocks: Vec<(usize, usize, usize)>,
    total_used: usize,
}

impl RwBuffer {
    pub fn new(initial_capacity: usize) -> Self {
        let mut blocks = Vec::new();
        if initial_capacity != 0 {
            blocks.push((0, initial_capacity.max(MIN_ALLOC_SIZE - BUFFER_HEAD_SIZE), 0));
        }
        RwBuffer { blocks, total_used: 0 }
    }

    pub fn size(&self) -> usize {
        self.total_used
    }

    pub fn append(&mut self, length: usize, reserve: usize) {
        if length == 0 {
            return;
        }
        let mut remaining = length;
        if self.blocks.is_empty() {
            self.blocks.push((self.total_used, (length + reserve).max(MIN_ALLOC_SIZE - BUFFER_HEAD_SIZE), 0));
        }
        let tail = self.blocks.last_mut().expect("tail block");
        let written = (tail.1 - tail.2).min(remaining);
        tail.2 += written;
        remaining -= written;
        self.total_used += written;
        if remaining != 0 {
            self.blocks.push((self.total_used, (remaining + reserve).max(MIN_ALLOC_SIZE - BUFFER_BLOCK_SIZE), remaining));
            self.total_used += remaining;
        }
    }

    pub fn snapshot(&self, bytes: &Rc<Vec<u8>>) -> Snapshot {
        let spans = self.blocks.iter().map(|&(start, capacity, _)| (start, start + capacity)).collect();
        Snapshot { bytes: bytes.clone(), size: self.total_used, spans: Rc::new(spans) }
    }
}
