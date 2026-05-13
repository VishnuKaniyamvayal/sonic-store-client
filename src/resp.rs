#[derive(Debug, Clone)]
pub enum RespType {
    SimpleString {
        data: String,
        delta: usize
    },
    Error {
        message: String,
        delta: usize
    },
    Integer {
        data: i64,
        delta: usize
    },
    BulkString {
        data: Vec<u8>,
        delta: usize
    },
    Array {
        data: Vec<RespType>,
        delta: usize
    },
}
fn read_simple_string(data: &[u8]) -> Result<RespType, RespType> {
    let mut pos: usize = 0;
    let mut return_data: String = String::new();
    while data[pos] != b'\r' {
        return_data.push(data[pos] as char);
        pos += 1;
    }
    return Ok(RespType::SimpleString { data: return_data, delta: pos + 2 })
}

fn read_error(data: &[u8]) -> Result<RespType, RespType> {
    let mut pos: usize = 0;
    let mut error_message: String = String::new();
    while data[pos] != b'\r' {
        error_message.push(data[pos] as char);
        pos += 1;
    }
    return Ok(RespType::Error { message: error_message, delta: pos + 2 })
} 

fn read_int_64(data: &[u8]) -> Result<RespType, RespType> {
    let mut pos: usize = 0;
    let mut return_value: i64 = 0;
    let negative = data[pos] == b'-';
    if negative || data[pos] == b'+' {
        pos += 1;
    }
    while data[pos] != b'\r' {
        return_value = return_value * 10 + (data[pos] - b'0') as i64;
        pos += 1;
    }
    if negative {
        return_value = -return_value;
    }
    return Ok(RespType::Integer { data: return_value, delta: pos + 2 })
}

fn read_length(data: &[u8]) -> (usize, usize) {
    let mut pos: usize = 0;
    let mut length: usize = 0;
    while data[pos] != b'\r' {
        length = length * 10 + (data[pos] - b'0') as usize;
        pos += 1;
    }
    (length, pos + 2) // pos + 2 to skip \r\n
}

fn read_bulk_string(data: &[u8]) -> Result<RespType, RespType> {
    let (length, pos) = read_length(data);
    let pos = pos; // pos is now after \r\n
    let return_data = data[pos..pos + length].to_vec();
    let delta = pos + length + 2; // +2 for trailing \r\n
    return Ok(RespType::BulkString { data: return_data, delta })
}

fn get_delta(resp: &RespType) -> usize {
    match resp {
        RespType::SimpleString { delta, .. } => *delta,
        RespType::Error { delta, .. } => *delta,
        RespType::Integer { delta, .. } => *delta,
        RespType::BulkString { delta, .. } => *delta,
        RespType::Array { delta, .. } => *delta,
    }
}

fn read_array(data: &[u8]) -> Result<RespType, RespType> {
    let (len, mut pos) = read_length(data);
    let mut return_data: Vec<RespType> = Vec::with_capacity(len);

    for _ in 0..len {
        let response = decode(&data[pos..])?;
        pos += get_delta(&response);
        return_data.push(response);
    }

    return Ok(RespType::Array { data: return_data, delta: pos })
}

fn add_to_delta(resp: &mut RespType, extra: usize) {
    match resp {
        RespType::SimpleString { delta, .. } => *delta += extra,
        RespType::Error { delta, .. } => *delta += extra,
        RespType::Integer { delta, .. } => *delta += extra,
        RespType::BulkString { delta, .. } => *delta += extra,
        RespType::Array { delta, .. } => *delta += extra,
    }
}

pub fn decode_arguments(data: &[u8]) -> Result<Vec<RespType>, RespType> {
    let mut pos: usize = 0;
    let mut tokens: Vec<RespType> = Vec::new();
    while pos < data.len() {
        let token = decode(&data[pos..])?;
        pos += get_delta(&token);
        match token {
            RespType::SimpleString { data, .. } => tokens.push(RespType::SimpleString { data, delta: 0 }),
            RespType::BulkString { data, .. } => tokens.push(RespType::BulkString { data, delta: 0 }),
            RespType::Integer { data, .. } => tokens.push(RespType::Integer { data, delta: 0 }),
            RespType::Array { data, .. } => {
                for item in data {
                    tokens.push(item);
                }
            },
            _ => return Err(RespType::Error { message: "Unsupported type in arguments".to_string(), delta: 0 }),
        }
    }
    Ok(tokens)
}

pub fn decode(data: &[u8]) -> Result<RespType, RespType> {
    if data.len() == 0 {
        return Err(RespType::Error { message: "Empty data".to_string(), delta: 0 });
    }
    match data[0] {
        b'+' => {
            let mut result = read_simple_string(&data[1..])?;
            add_to_delta(&mut result, 1);
            return Ok(result);
        }
        b'-' => {
            let mut result = read_error(&data[1..])?;
            add_to_delta(&mut result, 1);
            return Ok(result);
        }
        b':' => {
            let mut result = read_int_64(&data[1..])?;
            add_to_delta(&mut result, 1);
            return Ok(result);
        }
        b'$' => {
            let mut result = read_bulk_string(&data[1..])?;
            add_to_delta(&mut result, 1);
            return Ok(result);
        } 
        b'*' => {
            let mut result = read_array(&data[1..])?;
            add_to_delta(&mut result, 1);
            return Ok(result);
        }
        _ => {
            return Err(RespType::Error { message: "Invalid character Error".to_string(), delta: 0 });
        }
    }
}