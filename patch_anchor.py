#!/usr/bin/env python3
"""Patch: reminder anchor — recurring reschedule dari anchor jam asli.
Idempotent (batal kalau sudah applied). Pola anchored-replace eksak."""
import sys, pathlib

root = pathlib.Path("/root/my-assist")
rem = root / "src/reminders.rs"
gw = root / "src/gateway.rs"

src = rem.read_text()
if "anchor_at" in src:
    print("SKIP: anchor_at sudah ada di reminders.rs")
    sys.exit(0)

def rep(text, old, new, count):
    n = text.count(old)
    assert n == count, f"expect {count}x, found {n}x: {old[:60]!r}"
    return text.replace(old, new)

# 1. struct Reminder + field anchor
src = rep(src, """    pub recur: Option<String>,
    pub fail_count: i32,
}""", """    pub recur: Option<String>,
    pub fail_count: i32,
    /// Anchor jadwal asli (jam saat dibuat) — recurring di-reschedule dari sini
    /// supaya backoff/fire telat tidak menggeser HH:MM (bug drift 07:50→08:56).
    pub anchor_at: Option<DateTime<Utc>>,
}""", 1)

# 2. create() set anchor_at = remind_at
src = rep(src, """        "INSERT INTO reminders (chat_id, message, remind_at, kind, recur)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",""",
"""        "INSERT INTO reminders (chat_id, message, remind_at, kind, recur, anchor_at)
         VALUES ($1, $2, $3, $4, $5, $3) RETURNING id",""", 1)

# 3. tuple query_as (due_now + list_pending)
src = rep(src,
"sqlx::query_as::<_, (i64, i64, String, DateTime<Utc>, String, Option<String>, i32)>(",
"sqlx::query_as::<_, (i64, i64, String, DateTime<Utc>, String, Option<String>, i32, Option<DateTime<Utc>>)>(",
2)

# 4. SELECT + anchor_at (due_now + list_pending)
src = rep(src, """        "SELECT id, chat_id, message, remind_at, kind, recur, fail_count
         FROM reminders""",
"""        "SELECT id, chat_id, message, remind_at, kind, recur, fail_count, anchor_at
         FROM reminders""", 2)

# 5. map closure (due_now + list_pending)
src = rep(src,
".map(|(id, chat_id, message, remind_at, kind, recur, fail_count)| Reminder {",
".map(|(id, chat_id, message, remind_at, kind, recur, fail_count, anchor_at)| Reminder {",
2)

# 6. struct literal close (due_now + list_pending)
src = rep(src, """            fail_count,
        })""", """            fail_count,
            anchor_at,
        })""", 2)

# 7. compute_next_run — anchored + catch-up
src = rep(src, """pub fn compute_next_run(recur: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
    match recur {
        "daily" => Some(from + Duration::days(1)),
        "weekly" => Some(from + Duration::weeks(1)),
        _ => None,
    }
}""",
"""pub fn compute_next_run(
    recur: &str,
    anchor: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let period = match recur {
        "daily" => Duration::days(1),
        "weekly" => Duration::weeks(1),
        _ => return None,
    };
    // Anchor ke jam asli: next = occurrence anchor + k*period pertama yang > now.
    // Fire telat (backoff / service down) tidak menggeser HH:MM — occurrence
    // yang terlewat dilompati (catch-up), anchor tetap.
    let mut next = anchor;
    while next <= now {
        next += period;
    }
    Some(next)
}""", 1)

# 8. tests baru
src = rep(src, """        assert!(cumulative_backoff_secs(8) > GIVEUP_AFTER_SECS);
    }
}""",
"""        assert!(cumulative_backoff_secs(8) > GIVEUP_AFTER_SECS);
    }

    fn fixed(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn daily_anchor_tetap_walau_fire_telat() {
        // Anchor 07:50 WIB = 00:50 UTC; fire telat 1 jam (backoff/down).
        let anchor = fixed("2026-08-31T00:50:00Z");
        let next = compute_next_run("daily", anchor, anchor + Duration::hours(1)).unwrap();
        // Tetap 00:50 besok — bukan ikut geser ke 01:50.
        assert_eq!(next, fixed("2026-09-01T00:50:00Z"));
    }

    #[test]
    fn daily_catch_up_setelah_down_lama() {
        let anchor = fixed("2026-08-31T00:50:00Z");
        // Down 3 hari, bangun 2 jam setelah occurrence hari ke-3.
        let now = anchor + Duration::days(3) + Duration::hours(2);
        let next = compute_next_run("daily", anchor, now).unwrap();
        assert_eq!(next, fixed("2026-09-04T00:50:00Z"));
    }

    #[test]
    fn weekly_anchor_dan_recur_tak_dikenal() {
        let anchor = fixed("2026-08-31T00:50:00Z");
        let next = compute_next_run("weekly", anchor, anchor).unwrap();
        assert_eq!(next, fixed("2026-09-07T00:50:00Z"));
        assert!(compute_next_run("hourly", anchor, anchor).is_none());
    }
}""", 1)

rem.write_text(src)

# 9. gateway.rs — hapus original_remind_at, reschedule dari anchor
gw_src = gw.read_text()
gw_src = rep(gw_src, "        let original_remind_at = r.remind_at;\n", "", 1)
gw_src = rep(gw_src,
"""                    .and_then(|rec| reminders::compute_next_run(rec, original_remind_at))""",
"""                    .and_then(|rec| {
                        reminders::compute_next_run(rec, r.anchor_at.unwrap_or(r.remind_at), Utc::now())
                    })""", 1)
gw.write_text(gw_src)

print("PATCH OK: anchor_at (reminders.rs + gateway.rs)")
