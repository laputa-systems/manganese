pub const NULL_BYTE: usize = usize::MAX;

#[derive(Clone, Copy, Debug)]
pub struct SourceState {
    pub next_input_byte: usize,
    pub bytes_in_buffer: usize,
}

impl Default for SourceState {
    fn default() -> Self {
        SourceState { next_input_byte: NULL_BYTE, bytes_in_buffer: 0 }
    }
}

pub trait Source {
    fn data(&self) -> &[u8];
    fn fill_input_buffer(&mut self, state: &mut SourceState) -> bool;
    fn skip_input_data(&mut self, state: &mut SourceState, num_bytes: i64);
    fn term_source(&mut self);
}
