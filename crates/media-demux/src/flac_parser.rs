use std::collections::VecDeque;

use crate::bits::BitReader;
use crate::flac;
use crate::flac::Crc16;
use crate::flac::FrameInfo;
use crate::format::CodecParams;
use crate::parser::ParserCommon;

const MAX_SEQUENTIAL_HEADERS: usize = 4;
const MIN_HEADERS: i32 = 10;
const AVG_FRAME_SIZE: usize = 8192;
const HEADER_BASE_SCORE: i32 = 10;
const HEADER_CHANGED_PENALTY: i32 = 7;
const HEADER_CRC_FAIL_PENALTY: i32 = 50;
const HEADER_NOT_PENALIZED_YET: i32 = 100000;
const HEADER_NOT_SCORED_YET: i32 = -100000;
const MAX_FRAME_HEADER_SIZE: usize = 16;
const MAX_FRAME_VERIFY_SIZE: usize = MAX_FRAME_HEADER_SIZE + 1;
const PADDING: usize = 64;

#[derive(Clone)]
struct Marker {
    id: u64,
    offset: i32,
    link_penalty: [i32; MAX_SEQUENTIAL_HEADERS],
    max_score: i32,
    fi: FrameInfo,
    best_child: Option<u64>,
}

pub struct FlacParser {
    headers: VecDeque<Marker>,
    next_id: u64,
    best_header: Option<u64>,
    nb_headers_found: i32,
    nb_headers_buffered: i32,
    best_header_valid: bool,
    fifo: Vec<u8>,
    fifo_capacity: usize,
    end_padded: bool,
    last_fi: FrameInfo,
    last_fi_valid: bool,
}

pub struct Parsed {
    pub index: i32,
    pub frame: Option<Vec<u8>>,
}

fn frame_header_is_valid(buf: &[u8]) -> Option<FrameInfo> {
    let mut gb = BitReader::new(buf, MAX_FRAME_VERIFY_SIZE * 8);
    let fi = flac::decode_frame_header(&mut gb)?;
    if gb.get1() {
        return None;
    }
    let subframe_type = gb.get(6);
    if !(subframe_type == 0 || subframe_type == 1 || (8..=12).contains(&subframe_type) || subframe_type >= 32) {
        return None;
    }
    Some(fi)
}

fn check_header_fi_mismatch(header: &FrameInfo, child: &FrameInfo) -> i32 {
    let mut deduction = 0;
    if child.samplerate != header.samplerate {
        deduction += HEADER_CHANGED_PENALTY;
    }
    if child.bps != header.bps {
        deduction += HEADER_CHANGED_PENALTY;
    }
    if child.is_var_size != header.is_var_size {
        deduction += HEADER_BASE_SCORE;
    }
    if child.channels != header.channels {
        deduction += HEADER_CHANGED_PENALTY;
    }
    deduction
}

impl FlacParser {
    pub fn new() -> Self {
        FlacParser {
            headers: VecDeque::new(),
            next_id: 0,
            best_header: None,
            nb_headers_found: 0,
            nb_headers_buffered: 0,
            best_header_valid: false,
            fifo: Vec::new(),
            fifo_capacity: (MIN_HEADERS as usize + 3) * AVG_FRAME_SIZE,
            end_padded: false,
            last_fi: FrameInfo::default(),
            last_fi_valid: false,
        }
    }

    fn position(&self, id: u64) -> usize {
        let first = self.headers.front().expect("a referenced header is buffered").id;
        (id - first) as usize
    }

    fn marker(&self, id: u64) -> &Marker {
        &self.headers[self.position(id)]
    }

    fn next_of(&self, id: u64) -> Option<u64> {
        self.headers.get(self.position(id) + 1).map(|marker| marker.id)
    }

    fn fifo_write(&mut self, data: &[u8]) {
        let space = self.fifo_capacity - self.fifo.len();
        if space < data.len() {
            self.fifo_capacity += self.fifo.len().max(data.len());
        }
        self.fifo.extend_from_slice(data);
    }

    fn validate(&mut self, offset: usize) -> i32 {
        let end = (offset + MAX_FRAME_VERIFY_SIZE + PADDING).min(self.fifo.len());
        let Some(fi) = frame_header_is_valid(&self.fifo[offset..end]) else {
            return 0;
        };
        let id = self.next_id;
        self.next_id += 1;
        self.headers.push_back(Marker {
            id,
            offset: offset as i32,
            link_penalty: [HEADER_NOT_PENALIZED_YET; MAX_SEQUENTIAL_HEADERS],
            max_score: 0,
            fi,
            best_child: None,
        });
        self.nb_headers_found += 1;
        self.headers.len() as i32
    }

    fn find_new_headers(&mut self, search_start: i64) -> i32 {
        self.nb_headers_found = 0;
        let search_end = self.fifo.len() as i64 - (MAX_FRAME_HEADER_SIZE as i64 - 1);
        let mut size = 0;
        let mut position = search_start;
        while position < search_end {
            let at = position as usize;
            if ((self.fifo[at] as u32) << 8 | self.fifo[at + 1] as u32) & 0xFFFE == 0xFFF8 {
                size = size.max(self.validate(at));
            }
            position += 1;
        }
        if size == 0 && !self.headers.is_empty() {
            size = self.headers.len() as i32;
        }
        size
    }

    fn crc_zero(&self, start: i32, end: i32) -> bool {
        let mut crc = Crc16::new();
        crc.update(&self.fifo[start as usize..end as usize]);
        crc.value() == 0
    }

    fn check_header_mismatch(&self, header_id: u64, child_id: u64) -> i32 {
        let header = self.marker(header_id);
        let child = self.marker(child_id);
        let mut deduction = check_header_fi_mismatch(&header.fi, &child.fi);
        let mut deduction_expected = 0;
        if child.fi.frame_or_sample_num.wrapping_sub(header.fi.frame_or_sample_num) != header.fi.blocksize as i64
            && child.fi.frame_or_sample_num != header.fi.frame_or_sample_num.wrapping_add(1)
        {
            let mut expected_frame_num = header.fi.frame_or_sample_num;
            let mut expected_sample_num = header.fi.frame_or_sample_num;
            let mut curr = header_id;
            while curr != child_id {
                let marker = self.marker(curr);
                for i in 0..MAX_SEQUENTIAL_HEADERS {
                    if marker.link_penalty[i] < HEADER_CRC_FAIL_PENALTY {
                        expected_frame_num = expected_frame_num.wrapping_add(1);
                        expected_sample_num = expected_sample_num.wrapping_add(marker.fi.blocksize as i64);
                        break;
                    }
                }
                curr = self.next_of(curr).expect("the child follows the header");
            }
            if expected_frame_num == child.fi.frame_or_sample_num || expected_sample_num == child.fi.frame_or_sample_num {
                deduction_expected = if deduction != 0 { 0 } else { 1 };
            }
            deduction += HEADER_CHANGED_PENALTY;
        }
        let check_crc = if self.last_fi.is_var_size == header.fi.is_var_size {
            if self.last_fi.is_var_size
                && self.last_fi.frame_or_sample_num.wrapping_add(self.last_fi.blocksize as i64) == header.fi.frame_or_sample_num
            {
                false
            } else if !self.last_fi.is_var_size && self.last_fi.frame_or_sample_num.wrapping_add(1) == header.fi.frame_or_sample_num {
                false
            } else {
                deduction == 0 && deduction_expected == 0
            }
        } else {
            deduction == 0 && deduction_expected == 0
        };
        if check_crc || (deduction != 0 && deduction_expected == 0) {
            let mut crc_is_zero = false;
            let mut inverted_test = false;
            let mut curr = self.next_of(header_id).expect("the child follows the header");
            let mut i = 0;
            while i < MAX_SEQUENTIAL_HEADERS && curr != child_id {
                curr = self.next_of(curr).expect("the child follows the header");
                i += 1;
            }
            assert!(i < MAX_SEQUENTIAL_HEADERS, "a scored child is at most four headers ahead");
            if header.link_penalty[i] < HEADER_CRC_FAIL_PENALTY || header.link_penalty[i] == HEADER_NOT_PENALIZED_YET {
                let mut start = header_id;
                let mut end = child_id;
                let next = self.next_of(header_id).expect("the child follows the header");
                if i > 0 && header.link_penalty[i - 1] >= HEADER_CRC_FAIL_PENALTY {
                    while self.next_of(start) != Some(child_id) {
                        start = self.next_of(start).expect("the child follows the header");
                    }
                    inverted_test = true;
                } else if i > 0 && self.marker(next).link_penalty[i - 1] >= HEADER_CRC_FAIL_PENALTY {
                    end = next;
                    inverted_test = true;
                }
                crc_is_zero = self.crc_zero(self.marker(start).offset, self.marker(end).offset);
            }
            if crc_is_zero == inverted_test {
                deduction += HEADER_CRC_FAIL_PENALTY;
            }
        }
        deduction
    }

    fn score_header(&mut self, id: u64) -> i32 {
        let at = self.position(id);
        if self.headers[at].max_score != HEADER_NOT_SCORED_YET {
            return self.headers[at].max_score;
        }
        let mut base_score = HEADER_BASE_SCORE;
        if self.last_fi_valid {
            base_score -= check_header_fi_mismatch(&self.last_fi, &self.headers[at].fi);
        }
        self.headers[at].max_score = base_score;
        let mut child = self.next_of(id);
        let mut dist = 0;
        while dist < MAX_SEQUENTIAL_HEADERS {
            let Some(child_id) = child else {
                break;
            };
            if self.headers[at].link_penalty[dist] == HEADER_NOT_PENALIZED_YET {
                let penalty = self.check_header_mismatch(id, child_id);
                self.headers[at].link_penalty[dist] = penalty;
            }
            let child_score = self.score_header(child_id) - self.headers[at].link_penalty[dist];
            if HEADER_BASE_SCORE + child_score > self.headers[at].max_score {
                self.headers[at].best_child = Some(child_id);
                self.headers[at].max_score = base_score + child_score;
            }
            child = self.next_of(child_id);
            dist += 1;
        }
        self.headers[at].max_score
    }

    fn score_sequences(&mut self) {
        let mut best_score = HEADER_NOT_SCORED_YET;
        for marker in &mut self.headers {
            marker.max_score = HEADER_NOT_SCORED_YET;
        }
        let ids: Vec<u64> = self.headers.iter().map(|marker| marker.id).collect();
        for id in ids {
            if self.score_header(id) > best_score {
                self.best_header = Some(id);
                best_score = self.marker(id).max_score;
            }
        }
    }

    fn get_best_header(&mut self, common: &mut ParserCommon, avctx: &mut CodecParams) -> Parsed {
        let header_id = self.best_header.expect("a best header was chosen");
        let header = self.marker(header_id).clone();
        let child = header.best_child.map(|id| self.marker(id).clone());
        let size = match &child {
            None => self.fifo.len() as i32 - header.offset,
            Some(child) => child.offset - header.offset,
        };
        flac::set_channel_layout(avctx, header.fi.channels);
        avctx.sample_rate = header.fi.samplerate;
        common.duration = header.fi.blocksize;
        let frame = self.fifo[header.offset as usize..(header.offset + size) as usize].to_vec();
        if common.use_codec_ts {
            if header.fi.is_var_size {
                common.pts = header.fi.frame_or_sample_num;
            } else if header.best_child.is_some() {
                common.pts = header.fi.frame_or_sample_num.wrapping_mul(header.fi.blocksize as i64);
            }
        }
        self.best_header_valid = false;
        self.last_fi_valid = true;
        self.last_fi = header.fi;
        let mut index = 0;
        if let Some(child) = child {
            let offset = child.offset as i64 - self.fifo.len() as i64;
            if offset > -(1 << 28) {
                index = offset as i32;
            }
        }
        Parsed { index, frame: if size > 0 { Some(frame) } else { None } }
    }

    fn remove_front_until(&mut self, stop: u64) {
        while let Some(front) = self.headers.front() {
            if front.id == stop {
                break;
            }
            self.headers.pop_front();
            self.nb_headers_buffered -= 1;
        }
    }

    pub fn parse(&mut self, common: &mut ParserCommon, avctx: &mut CodecParams, buf: &[u8]) -> Parsed {
        let buf_size = buf.len();
        if common.complete_frames {
            let mut padded = buf[..buf_size.min(MAX_FRAME_VERIFY_SIZE)].to_vec();
            padded.resize(MAX_FRAME_VERIFY_SIZE, 0);
            if let Some(fi) = frame_header_is_valid(&padded) {
                common.duration = fi.blocksize;
                if avctx.sample_rate == 0 {
                    avctx.sample_rate = fi.samplerate;
                }
                if common.use_codec_ts {
                    common.pts = fi.frame_or_sample_num;
                    if !fi.is_var_size {
                        common.pts = common.pts.wrapping_mul(fi.blocksize as i64);
                    }
                }
            }
            return Parsed { index: buf_size as i32, frame: if buf_size > 0 { Some(buf.to_vec()) } else { None } };
        }
        if self.best_header_valid && self.nb_headers_buffered >= MIN_HEADERS {
            return self.get_best_header(common, avctx);
        }
        let best_child = self.best_header.and_then(|id| self.marker(id).best_child);
        if let Some(best_child) = best_child {
            self.remove_front_until(best_child);
            let drained = self.headers.front().expect("the best child stays buffered").offset;
            self.fifo.drain(..drained as usize);
            for marker in &mut self.headers {
                marker.offset -= drained;
            }
            if self.nb_headers_buffered >= MIN_HEADERS {
                self.best_header = Some(best_child);
                return self.get_best_header(common, avctx);
            }
            self.best_header = None;
        } else if let Some(best) = self.best_header {
            self.remove_front_until(best);
            self.headers.pop_front();
            self.best_header = None;
            self.nb_headers_buffered -= 1;
        }
        let mut read_start = 0usize;
        let mut read_end = 0usize;
        let mut read_end_null = false;
        let mut error = false;
        while (buf_size > 0 && read_end < buf_size && self.nb_headers_buffered < MIN_HEADERS) || (buf_size == 0 && !self.end_padded) {
            if buf_size == 0 {
                self.end_padded = true;
                read_end = read_start + MAX_FRAME_HEADER_SIZE;
            } else {
                let nb_desired = (MIN_HEADERS - self.nb_headers_buffered + 1) as usize;
                read_end += (buf_size - read_end).min(nb_desired * AVG_FRAME_SIZE);
            }
            if self.fifo_capacity == self.fifo.len() && self.fifo.len() / AVG_FRAME_SIZE > (self.nb_headers_buffered as i64 * 20) as usize {
                error = true;
                break;
            }
            if buf_size > 0 {
                let chunk = buf[read_start..read_end].to_vec();
                self.fifo_write(&chunk);
            } else {
                self.fifo_write(&[0; MAX_FRAME_HEADER_SIZE]);
            }
            let start_offset = (self.fifo.len() as i64 - ((read_end - read_start) as i64 + (MAX_FRAME_HEADER_SIZE as i64 - 1))).max(0);
            let nb_headers = self.find_new_headers(start_offset);
            self.nb_headers_buffered = nb_headers;
            if !self.end_padded && self.nb_headers_buffered < MIN_HEADERS {
                if read_end < buf_size {
                    read_start = read_end;
                    continue;
                }
                error = true;
                break;
            }
            if self.end_padded || self.nb_headers_found != 0 {
                self.score_sequences();
            }
            if self.end_padded {
                let size = self.fifo.len();
                self.fifo.truncate(size - MAX_FRAME_HEADER_SIZE);
                read_end_null = true;
                read_start = 0;
                read_end = 0;
            }
        }
        if !error {
            for marker in &self.headers {
                let better = match self.best_header {
                    None => true,
                    Some(best) => marker.max_score > self.marker(best).max_score,
                };
                if better {
                    self.best_header = Some(marker.id);
                }
            }
            if let Some(best) = self.best_header
                && self.marker(best).max_score <= 0
                && (buf_size == 0 || read_end_null || read_end != 0 || self.nb_headers_buffered < MIN_HEADERS)
            {
                self.best_header = None;
            }
            if let Some(best) = self.best_header {
                self.best_header_valid = true;
                let offset = self.marker(best).offset;
                if offset > 0 {
                    common.duration = 0;
                    let frame = self.fifo[..offset as usize].to_vec();
                    let index = if buf_size > 0 { read_end as i32 } else { offset - self.fifo.len() as i32 };
                    return Parsed { index, frame: Some(frame) };
                }
                if buf_size == 0 {
                    return self.get_best_header(common, avctx);
                }
            }
        }
        Parsed { index: if buf_size > 0 { read_end as i32 } else { 0 }, frame: None }
    }
}
