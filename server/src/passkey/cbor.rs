/// CBOR parsing utilities for WebAuthn attestation and assertion data.
/// These functions implement minimal CBOR decoding needed for passkey operations.

/// Read a CBOR integer value from the data stream.
/// Supports unsigned integers up to 64 bits.
pub fn read_int(data: &[u8], offset: &mut usize) -> Option<i64> {
    if *offset >= data.len() {
        return None;
    }
    let first = data[*offset];
    *offset += 1;
    match first {
        0..=23 => Some(first as i64),
        24 => {
            if *offset >= data.len() {
                None
            } else {
                let val = data[*offset] as i64;
                *offset += 1;
                Some(val)
            }
        }
        25 => {
            if *offset + 1 >= data.len() {
                None
            } else {
                let val = u16::from_be_bytes([data[*offset], data[*offset + 1]]) as i64;
                *offset += 2;
                Some(val)
            }
        }
        26 => {
            if *offset + 3 >= data.len() {
                None
            } else {
                let val = u32::from_be_bytes([
                    data[*offset],
                    data[*offset + 1],
                    data[*offset + 2],
                    data[*offset + 3],
                ]) as i64;
                *offset += 4;
                Some(val)
            }
        }
        27 => {
            if *offset + 7 >= data.len() {
                None
            } else {
                let val = u64::from_be_bytes([
                    data[*offset],
                    data[*offset + 1],
                    data[*offset + 2],
                    data[*offset + 3],
                    data[*offset + 4],
                    data[*offset + 5],
                    data[*offset + 6],
                    data[*offset + 7],
                ]) as i64;
                *offset += 8;
                Some(val)
            }
        }
        _ => None,
    }
}

/// Read a CBOR byte string from the data stream.
pub fn read_bytes(data: &[u8], offset: &mut usize) -> Option<Vec<u8>> {
    if *offset >= data.len() {
        return None;
    }
    let first = data[*offset];
    *offset += 1;
    match first {
        0..=23 => {
            let len = first as usize;
            if *offset + len > data.len() {
                return None;
            }
            let result = data[*offset..*offset + len].to_vec();
            *offset += len;
            Some(result)
        }
        64 => None,
        65 => {
            if *offset >= data.len() {
                return None;
            }
            let len = data[*offset] as usize;
            *offset += 1;
            if *offset + len > data.len() {
                return None;
            }
            let result = data[*offset..*offset + len].to_vec();
            *offset += len;
            Some(result)
        }
        _ => None,
    }
}
