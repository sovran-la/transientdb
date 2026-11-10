//! CRC-32 as used by zlib and PNG: polynomial `0xEDB88320`, reflected, by table lookup.

/// The remainder of every byte value, built at compile time.
const TABLE: [u32; 256] = build_table();

const fn build_table() -> [u32; 256] {
	let mut table = [0u32; 256];
	let mut n = 0;
	while n < 256 {
		let mut c = n as u32;
		let mut bit = 0;
		while bit < 8 {
			c = if c & 1 == 1 {
				0xEDB8_8320 ^ (c >> 1)
			} else {
				c >> 1
			};
			bit += 1;
		}
		table[n] = c;
		n += 1;
	}
	table
}

/// A CRC-32 computed over several pieces of input in turn.
#[derive(Debug, Clone, Copy)]
pub(super) struct Crc32(u32);

impl Crc32 {
	pub(super) fn new() -> Self {
		Self(0xFFFF_FFFF)
	}

	pub(super) fn update(mut self, bytes: &[u8]) -> Self {
		for &byte in bytes {
			self.0 = TABLE[((self.0 ^ u32::from(byte)) & 0xFF) as usize] ^ (self.0 >> 8);
		}
		self
	}

	pub(super) fn finish(self) -> u32 {
		self.0 ^ 0xFFFF_FFFF
	}
}

/// The CRC-32 of `bytes`.
pub(super) fn crc32(bytes: &[u8]) -> u32 {
	Crc32::new().update(bytes).finish()
}
