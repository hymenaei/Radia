use super::error::LLUdpError;

pub(crate) fn encode(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len());
    let mut cursor = 0;
    while cursor < input.len() {
        if input[cursor] != 0 {
            output.push(input[cursor]);
            cursor += 1;
            continue;
        }

        let start = cursor;
        while cursor < input.len() && input[cursor] == 0 && cursor - start < u8::MAX as usize {
            cursor += 1;
        }
        output.extend_from_slice(&[0, (cursor - start) as u8]);
    }
    output
}

fn read_zero_count(input: &[u8], cursor: &mut usize) -> Result<usize, LLUdpError> {
    let count = *input.get(*cursor).ok_or(LLUdpError::InvalidZeroCode)?;
    *cursor += 1;
    if count != 0 {
        return Ok(usize::from(count));
    }

    let extension = *input.get(*cursor).ok_or(LLUdpError::InvalidZeroCode)?;
    *cursor += 1;
    Ok(usize::from(u8::MAX) + usize::from(extension))
}

pub(crate) fn decode_prefix(input: &[u8], output: &mut [u8]) -> Result<usize, LLUdpError> {
    let mut input_cursor = 0;
    let mut output_cursor = 0usize;
    while output_cursor < output.len() && input_cursor < input.len() {
        let byte = input[input_cursor];
        input_cursor += 1;
        if byte == 0 {
            let count = read_zero_count(input, &mut input_cursor)?.min(output.len() - output_cursor);
            output[output_cursor..output_cursor + count].fill(0);
            output_cursor += count;
        } else {
            output[output_cursor] = byte;
            output_cursor += 1;
        }
    }
    Ok(output_cursor)
}

#[cfg(test)]
pub(crate) fn decode(input: &[u8], max_output_size: usize) -> Result<Vec<u8>, LLUdpError> {
    let decoded_len = decoded_len(input, max_output_size)?;
    let mut output = Vec::with_capacity(decoded_len);
    decode_into(input, &mut output, decoded_len)?;
    Ok(output)
}

#[cfg(test)]
pub(crate) fn decoded_len(input: &[u8], max_output_size: usize) -> Result<usize, LLUdpError> {
    let mut cursor = 0;
    let mut output_len = 0usize;
    while cursor < input.len() {
        let byte = input[cursor];
        cursor += 1;
        if byte == 0 {
            let count = read_zero_count(input, &mut cursor)?;
            let output_size = output_len.checked_add(count).ok_or(LLUdpError::InvalidZeroCode)?;
            if output_size > max_output_size {
                return Err(LLUdpError::InvalidZeroCode);
            }
            output_len = output_size;
        } else {
            if output_len == max_output_size {
                return Err(LLUdpError::InvalidZeroCode);
            }
            output_len += 1;
        }
    }
    Ok(output_len)
}

pub(crate) fn decode_into(input: &[u8], output: &mut Vec<u8>, expected_len: usize) -> Result<(), LLUdpError> {
    output.clear();
    output.reserve_exact(expected_len);
    let mut cursor = 0;
    while cursor < input.len() {
        let byte = input[cursor];
        cursor += 1;
        if byte == 0 {
            let count = read_zero_count(input, &mut cursor)?;
            let output_size = output.len().checked_add(count).ok_or(LLUdpError::InvalidZeroCode)?;
            if output_size > expected_len {
                return Err(LLUdpError::InvalidZeroCode);
            }
            output.resize(output_size, 0);
        } else {
            if output.len() == expected_len {
                return Err(LLUdpError::InvalidZeroCode);
            }
            output.push(byte);
        }
    }
    if output.len() != expected_len {
        return Err(LLUdpError::InvalidZeroCode);
    }
    Ok(())
}

pub(crate) fn decode_to_slice(input: &[u8], output: &mut [u8]) -> Result<usize, LLUdpError> {
    let mut cursor = 0;
    let mut output_cursor = 0usize;
    while cursor < input.len() {
        let byte = input[cursor];
        cursor += 1;
        if byte == 0 {
            let count = read_zero_count(input, &mut cursor)?;
            let end = output_cursor.checked_add(count).ok_or(LLUdpError::InvalidZeroCode)?;
            if end > output.len() {
                return Err(LLUdpError::InvalidZeroCode);
            }
            output[output_cursor..end].fill(0);
            output_cursor = end;
        } else {
            let Some(slot) = output.get_mut(output_cursor) else {
                return Err(LLUdpError::InvalidZeroCode);
            };
            *slot = byte;
            output_cursor += 1;
        }
    }
    Ok(output_cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_runs_split_at_limit() {
        let input = vec![0; 256];
        let encoded = encode(&input);
        assert_eq!(encoded, [0, 255, 0, 1]);
        assert_eq!(decode(&encoded, input.len()), Ok(input));
        assert_eq!(decode(&[0], usize::MAX), Err(LLUdpError::InvalidZeroCode));
        assert_eq!(decode(&[0, 0], usize::MAX), Err(LLUdpError::InvalidZeroCode));
        assert_eq!(decode(&[0, 0, 0], 255), Ok(vec![0; 255]));
        assert_eq!(decode(&[0, 0, 1], 256), Ok(vec![0; 256]));
        assert_eq!(decode(&[0, 0, 255], 510), Ok(vec![0; 510]));
        assert_eq!(decode(&[0, 0, 255], 509), Err(LLUdpError::InvalidZeroCode));
        assert_eq!(decode(&[0, u8::MAX], usize::from(u8::MAX) - 1), Err(LLUdpError::InvalidZeroCode));
        assert_eq!(decode(&[1, 2], 1), Err(LLUdpError::InvalidZeroCode));
    }

    #[test]
    fn prefix_decode_is_bounded() {
        let mut output = [0; 4];
        assert_eq!(decode_prefix(&[0, u8::MAX, 0], &mut output), Ok(4));
        assert_eq!(output, [0; 4]);
        assert_eq!(decode_prefix(&[0], &mut output), Err(LLUdpError::InvalidZeroCode));
        assert_eq!(decode_prefix(&[0, 0], &mut output), Err(LLUdpError::InvalidZeroCode));
        assert_eq!(decode_prefix(&[0, 0, 1], &mut output), Ok(4));
    }
}
