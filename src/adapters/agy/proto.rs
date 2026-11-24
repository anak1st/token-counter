//! 零依赖 Protobuf wire 协议解码器（供 Antigravity `steps` / `gen_metadata` 解析使用）。

/// 从 protobuf wire 流解码出的单条顶层字段（字节切片零拷贝）。
pub(super) struct WireField<'a> {
    pub field: u32,
    pub varint: Option<u64>,
    pub bytes: Option<&'a [u8]>,
}

/// 顺序扫描 protobuf 二进制流，返回顶层字段列表；遇到异常 wire 类型则中止。
pub(super) fn decode_wire(buf: &[u8]) -> Vec<WireField<'_>> {
    let mut fields = Vec::new();
    let mut pos = 0usize;

    while pos < buf.len() {
        let tag = match read_varint(buf, &mut pos) {
            Some(v) => v,
            None => break,
        };
        let field = (tag >> 3) as u32;
        let wire = (tag & 7) as u8;
        if field == 0 {
            break;
        }

        match wire {
            0 => {
                let val = match read_varint(buf, &mut pos) {
                    Some(v) => v,
                    None => break,
                };
                fields.push(WireField {
                    field,
                    varint: Some(val),
                    bytes: None,
                });
            }
            1 => {
                if pos + 8 > buf.len() {
                    break;
                }
                pos += 8;
            }
            2 => {
                let len = match read_varint(buf, &mut pos) {
                    Some(v) => v as usize,
                    None => break,
                };
                if pos.saturating_add(len) > buf.len() {
                    break;
                }
                fields.push(WireField {
                    field,
                    varint: None,
                    bytes: Some(&buf[pos..pos + len]),
                });
                pos += len;
            }
            5 => {
                if pos + 4 > buf.len() {
                    break;
                }
                pos += 4;
            }
            _ => break,
        }
    }

    fields
}

/// 读取一个无符号 base-128 varint 并推进游标 `pos`。
fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut val = 0u64;
    let mut shift = 0u32;

    while *pos < buf.len() && shift < 64 {
        let b = buf[*pos];
        *pos += 1;
        val |= u64::from(b & 0x7f) << shift;
        if (b & 0x80) == 0 {
            return Some(val);
        }
        shift += 7;
    }

    None
}

/// 读取首个匹配的 length-delimited 字节字段。
pub(super) fn get_bytes<'a>(fields: &[WireField<'a>], num: u32) -> Option<&'a [u8]> {
    for item in fields {
        if item.field == num {
            if let Some(bytes) = item.bytes {
                return Some(bytes);
            }
        }
    }
    None
}

/// 读取首个匹配字段并按 UTF-8 转为字符串。
pub(super) fn get_string(fields: &[WireField<'_>], num: u32) -> Option<String> {
    let bytes = get_bytes(fields, num)?;
    Some(String::from_utf8_lossy(bytes).into_owned())
}

/// 读取首个匹配的 varint 无符号整数字段。
pub(super) fn get_u64(fields: &[WireField<'_>], num: u32) -> Option<u64> {
    for item in fields {
        if item.field == num {
            if let Some(v) = item.varint {
                return Some(v);
            }
        }
    }
    None
}

/// 解码 protobuf Timestamp / Duration（`f1 = seconds`, `f2 = nanos`）为毫秒；
/// 遇到空子消息返回 `None`，避免把缺失的 TTFT 错计为 0ms。
pub(super) fn parse_time_ms(bytes: Option<&[u8]>) -> Option<i64> {
    let bytes = bytes?;
    if bytes.is_empty() {
        return None;
    }
    let fields = decode_wire(bytes);
    let secs_opt = get_u64(&fields, 1);
    let nanos_opt = get_u64(&fields, 2);
    if secs_opt.is_none() && nanos_opt.is_none() {
        return None;
    }
    let seconds = secs_opt.unwrap_or(0) as i64;
    let nanos = nanos_opt.unwrap_or(0) as i64;
    Some(seconds.saturating_mul(1000) + (nanos + 500_000) / 1_000_000)
}
