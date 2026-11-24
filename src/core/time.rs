//! 基于 `jiff` 的本地时区时间区间与分桶键计算。
//!
//! 所有时间窗口统一采用本地时区的半开区间 `[start_ms, end_ms)`，
//! 符合用户对“今天 / 本周 / 本月 / 最近 N 小时”的直觉。

use anyhow::{bail, Context, Result};
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan, ZonedDateTime};

/// 单次报表的时间聚合粒度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granularity {
    Hourly,
    Daily,
    Weekly,
    Monthly,
}

impl Granularity {
    /// 返回表格首列的英文表头名。
    pub fn column_header(self) -> &'static str {
        match self {
            Granularity::Hourly => "Hour",
            Granularity::Daily => "Date",
            Granularity::Weekly => "Week",
            Granularity::Monthly => "Month",
        }
    }
}

/// 毫秒级半开时间区间 `[start_ms, end_ms)`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeRange {
    /// 起始时间戳（含），单位毫秒。
    pub start_ms: i64,
    /// 结束时间戳（不含），单位毫秒。
    pub end_ms: i64,
}

impl TimeRange {
    /// 覆盖全量历史的时间范围（供 session 全量列表与查找使用）。
    pub const ALL: TimeRange = TimeRange {
        start_ms: 0,
        end_ms: i64::MAX,
    };

    /// 构造覆盖最近 `count` 个分桶（含当前分桶）的本地时间区间。
    pub fn last_n(granularity: Granularity, count: u32) -> Result<Self> {
        let now = ZonedDateTime::now();
        Self::last_n_from(&now, granularity, count)
    }

    /// `last_n` 的确定性核心实现，接收显式参考时刻便于单元测试。
    pub fn last_n_from(
        now: &ZonedDateTime,
        granularity: Granularity,
        count: u32,
    ) -> Result<Self> {
        if count == 0 {
            bail!("bucket count must be >= 1");
        }
        let back = i64::from(count - 1);

        let (start, end) = match granularity {
            Granularity::Hourly => {
                let current_hour = now
                    .with()
                    .minute(0)
                    .second(0)
                    .subsec_nanosecond(0)
                    .build()
                    .context("failed to truncate current hour")?;
                let start = current_hour
                    .checked_sub(back.hours())
                    .context("failed to compute hourly start")?;
                let end = current_hour
                    .checked_add(1.hour())
                    .context("failed to compute hourly end")?;
                (start, end)
            }
            Granularity::Daily => {
                let today_start = now
                    .start_of_day()
                    .context("failed to compute start of day")?;
                let start = today_start
                    .checked_sub(back.days())
                    .context("failed to compute daily start")?;
                let end = today_start
                    .checked_add(1.day())
                    .context("failed to compute daily end")?;
                (start, end)
            }
            Granularity::Weekly => {
                let days_since_monday = i64::from(now.weekday().to_monday_zero_offset());
                let today_start = now
                    .start_of_day()
                    .context("failed to compute start of day")?;
                let week_start = today_start
                    .checked_sub(days_since_monday.days())
                    .context("failed to compute start of current week")?;
                let start = week_start
                    .checked_sub(back.weeks())
                    .context("failed to compute weekly start")?;
                let end = week_start
                    .checked_add(1.week())
                    .context("failed to compute weekly end")?;
                (start, end)
            }
            Granularity::Monthly => {
                let first_day = now.date().first_of_month();
                let month_start = first_day
                    .to_zoned_datetime(now.time_zone().clone())
                    .context("failed to compute start of current month")?;
                let start = month_start
                    .checked_sub(back.months())
                    .context("failed to compute monthly start")?;
                let end = month_start
                    .checked_add(1.month())
                    .context("failed to compute monthly end")?;
                (start, end)
            }
        };

        Ok(Self {
            start_ms: start.timestamp().as_millisecond(),
            end_ms: end.timestamp().as_millisecond(),
        })
    }
}

/// 将毫秒时间戳格式化为本地时区的分桶键。
pub fn bucket_key(ts_ms: i64, granularity: Granularity) -> String {
    bucket_key_in_tz(ts_ms, granularity, &TimeZone::system())
}

/// 将毫秒时间戳格式化为本地时区的 `YYYY-MM-DD HH:MM` 字符串（用于会话与轮次时间展示）。
pub fn format_datetime(ts_ms: i64) -> String {
    let ts = match Timestamp::from_millisecond(ts_ms) {
        Ok(ts) => ts,
        Err(_) => return "unknown".to_string(),
    };
    let zoned = ts.to_zoned_datetime(TimeZone::system());
    let date = zoned.date();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        date.year(),
        date.month(),
        date.day(),
        zoned.hour(),
        zoned.minute()
    )
}

/// 将毫秒时间戳按指定时区格式化为分桶键。
pub fn bucket_key_in_tz(ts_ms: i64, granularity: Granularity, tz: &TimeZone) -> String {
    let ts = match Timestamp::from_millisecond(ts_ms) {
        Ok(ts) => ts,
        Err(_) => return "unknown".to_string(),
    };
    let zoned = ts.to_zoned_datetime(tz.clone());
    let date = zoned.date();

    match granularity {
        Granularity::Hourly => {
            format!(
                "{:04}-{:02}-{:02} {:02}:00",
                date.year(),
                date.month(),
                date.day(),
                zoned.hour()
            )
        }
        Granularity::Daily => {
            format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
        }
        Granularity::Weekly => {
            let iso = date.iso_week_date();
            format!("{:04}-W{:02}", iso.year(), iso.week())
        }
        Granularity::Monthly => {
            format!("{:04}-{:02}", date.year(), date.month())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil;

    fn sample_now() -> ZonedDateTime {
        // 2026-11-23 16:45:30 +08:00（周一）
        let tz = TimeZone::fixed(jiff::tz::offset(8));
        civil::date(2026, 11, 23)
            .at(16, 45, 30, 0)
            .to_zoned_datetime(tz)
            .unwrap()
    }

    #[test]
    fn bucket_keys_format_all_granularities() {
        let tz = TimeZone::fixed(jiff::tz::offset(8));
        let now = sample_now();
        let ms = now.timestamp().as_millisecond();

        assert_eq!(
            bucket_key_in_tz(ms, Granularity::Hourly, &tz),
            "2026-11-23 16:00"
        );
        assert_eq!(bucket_key_in_tz(ms, Granularity::Daily, &tz), "2026-11-23");
        assert_eq!(bucket_key_in_tz(ms, Granularity::Weekly, &tz), "2026-W48");
        assert_eq!(bucket_key_in_tz(ms, Granularity::Monthly, &tz), "2026-11");
    }

    #[test]
    fn last_n_computes_exact_calendar_boundaries() {
        let tz = TimeZone::fixed(jiff::tz::offset(8));
        let now = sample_now();

        // 最近 7 天：2026-11-17 00:00 .. 2026-11-24 00:00
        let daily = TimeRange::last_n_from(&now, Granularity::Daily, 7).unwrap();
        assert_eq!(
            bucket_key_in_tz(daily.start_ms, Granularity::Daily, &tz),
            "2026-11-17"
        );
        assert_eq!(
            bucket_key_in_tz(daily.end_ms, Granularity::Daily, &tz),
            "2026-11-24"
        );

        // 最近 4 周：3 周前的周一（2026-11-02）.. 下周一（2026-11-30）
        let weekly = TimeRange::last_n_from(&now, Granularity::Weekly, 4).unwrap();
        assert_eq!(
            bucket_key_in_tz(weekly.start_ms, Granularity::Daily, &tz),
            "2026-11-02"
        );
        assert_eq!(
            bucket_key_in_tz(weekly.end_ms, Granularity::Daily, &tz),
            "2026-11-30"
        );

        // 最近 12 个月：2025-12-01 .. 2026-12-01
        let monthly = TimeRange::last_n_from(&now, Granularity::Monthly, 12).unwrap();
        assert_eq!(
            bucket_key_in_tz(monthly.start_ms, Granularity::Daily, &tz),
            "2025-12-01"
        );
        assert_eq!(
            bucket_key_in_tz(monthly.end_ms, Granularity::Daily, &tz),
            "2026-12-01"
        );

        // 0 分桶数应报错
        assert!(TimeRange::last_n_from(&now, Granularity::Daily, 0).is_err());
    }
}
