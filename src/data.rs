use bbqueue::{ArrayLength, AutoReleaseGrantR};
use core::convert::TryInto;

#[derive(Debug, PartialEq)]
pub enum CoilState {
    On = 0xFF00,
    Off = 0x0000,
}

#[derive(Debug, PartialEq)]
pub struct CoilStore<'a, S: ArrayLength<u8>> {
    data: AutoReleaseGrantR<'a, S>,
    count: usize,
}

impl<'a, S: ArrayLength<u8>> CoilStore<'a, S> {
    pub fn new(data: AutoReleaseGrantR<'a, S>, count: usize) -> CoilStore<'a, S> {
        CoilStore { data, count }
    }

    pub fn iter(&'a self) -> CoilIterator<'a> {
        CoilIterator {
            current: 0,
            data: &self.data[0..(self.count + (8 - 1)) / 8],
            count: self.count,
        }
    }

    pub fn len(&self) -> usize {
        self.count
    }
}

impl<'a, S: ArrayLength<u8>> IntoIterator for &'a CoilStore<'a, S> {
    type Item = CoilState;
    type IntoIter = CoilIterator<'a>;

    fn into_iter(self) -> Self::IntoIter {
        CoilIterator {
            current: 0,
            data: &self.data,
            count: self.count,
        }
    }
}

pub struct CoilIterator<'a> {
    current: usize,
    data: &'a [u8],
    count: usize,
}

impl<'a> Iterator for CoilIterator<'a> {
    type Item = CoilState;
    fn next(&mut self) -> Option<Self::Item> {
        if self.current < self.count {
            let byte = self.current / 8;
            let bit = self.current % 8;
            self.current += 1;
            Some(if self.data[byte] >> bit & 1 == 1 {
                CoilState::On
            } else {
                CoilState::Off
            })
        } else {
            None
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct RegisterStore<'a, S: ArrayLength<u8>> {
    data: AutoReleaseGrantR<'a, S>,
}

impl<'a, S: ArrayLength<u8>> RegisterStore<'a, S> {
    pub fn new(data: AutoReleaseGrantR<'a, S>) -> RegisterStore<'a, S> {
        RegisterStore { data }
    }

    pub fn iter(&'a self) -> impl Iterator<Item = u16> + 'a {
        self.data
            .chunks(2)
            .map(|s| u16::from_be_bytes(s.try_into().unwrap_or_default()))
    }

    pub fn len(&self) -> usize {
        self.data.len() / 2
    }
}
