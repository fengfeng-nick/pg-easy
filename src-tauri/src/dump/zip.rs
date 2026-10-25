//! 未压缩的 zip 容器：`schema.sql` 与 `data.sql`。

use crate::error::AppError;

use super::util::unix_now;

/// 一个未压缩（stored）的 zip，内含 `schema.sql` 和 `data.sql`。
pub(crate) fn build_zip(schema: &str, data: &str) -> Result<Vec<u8>, AppError> {
    let files = [
        ("schema.sql".to_string(), schema.as_bytes().to_vec()),
        ("data.sql".to_string(), data.as_bytes().to_vec()),
    ];
    let mut out = Vec::new();
    write_zip(&mut out, &files).map_err(|err| {
        AppError::new("io", "archive_failed").detail(err.to_string())
    })?;
    Ok(out)
}

fn write_zip(out: &mut Vec<u8>, files: &[(String, Vec<u8>)]) -> std::io::Result<()> {
    let mut central: Vec<u8> = Vec::new();
    let (dos_time, dos_date) = dos_now();
    for (name, content) in files {
        let crc = crc32(content);
        let offset = out.len() as u32;
        let name_bytes = name.as_bytes();

        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // names are UTF-8
        out.extend_from_slice(&0u16.to_le_bytes()); // stored
        out.extend_from_slice(&dos_time.to_le_bytes());
        out.extend_from_slice(&dos_date.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(content.len() as u32).to_le_bytes());
        out.extend_from_slice(&(content.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra field
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(content);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0x0800u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&dos_time.to_le_bytes());
        central.extend_from_slice(&dos_date.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(content.len() as u32).to_le_bytes());
        central.extend_from_slice(&(content.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }
    let count = files.len() as u16;
    let central_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(())
}

pub(crate) fn dos_now() -> (u16, u16) {
    let seconds = unix_now();
    let days = seconds / 86_400;
    let time = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let date = (((year.clamp(1980, 2107) - 1980) as u16) << 9)
        | ((month as u16) << 5)
        | day as u16;
    let dos_time = ((time / 3600) as u16) << 11
        | (((time % 3600) / 60) as u16) << 5
        | ((time % 60) / 2) as u16;
    (dos_time, date)
}

/// 1970-01-01 以来的天数转成公历日期（Howard Hinnant 算法）。
fn civil_from_days(days: u64) -> (i64, u32, u32) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
