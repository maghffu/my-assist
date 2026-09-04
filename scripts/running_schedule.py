#!/usr/bin/env python3
"""6KM Race Training Scheduler & Progress Tracker
Race day: 23 Agustus 2026
"""

import json
import os
import sys
from datetime import datetime, timezone, timedelta, timedelta

PROGRESS_FILE = "/opt/hermes-lite/data/running_progress.json"

SCHEDULE = {
    # Week 1 (mulai 6 Agustus)
    "2026-08-06": {
        "day": "Week 1 —  Kamis (6 Agu)",
        "type": "Tes Kemampuan",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "Lari santai 20 menit — jangan pedulikan pace",
            "Kalau capek: jalan 1 menit, lanjut lari lagi",
        ],
        "note": "Catat: berapa km, berapa kali berhenti, dan rasa capek (skala 1-10). Ini jadi patokan untuk minggu berikutnya.",
        "emoji": "🏃",
    },
    "2026-08-07": {
        "day": "Week 1 —  Jumat (7 Agu)",
        "type": "Istirahat + Stretching",
        "plan": [
            "Istirahat total dari lari",
            "Stretching ringan 15 menit (hamstring, quad, calf, hip flexor)",
        ],
        "note": "Saat istirahat, otot sedang memperbaiki diri. Jangan skip.",
        "emoji": "🧘",
    },
    "2026-08-08": {
        "day": "Week 1 —  Sabtu (8 Agu)",
        "type": "Strength Training — 20 min",
        "plan": [
            "2 ronde, tanpa beban:",
            "• 15 Squat",
            "• 10 Lunge tiap kaki",
            "• 15 Glute Bridge",
            "• 20 Calf Raise",
            "• Plank 30 detik",
            "Istirahat 60 detik antar ronde",
        ],
        "note": "Strength ringan 20 menit. Fokus form, bukan berat.",
        "emoji": "💪",
    },
    "2026-08-09": {
        "day": "Week 1 —  Minggu (9 Agu)",
        "type": "Easy Run — 3.5-4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "Lari 3.5-4 km santai",
            "Pace harus masih bisa ngobrol — kalau ngos-ngosan, terlalu cepat",
        ],
        "note": "Targetnya bukan cepat, tapi konsisten tanpa berhenti.",
        "emoji": "🏃",
    },
    "2026-08-10": {
        "day": "Week 1 —  Senin (10 Agu)",
        "type": "Long Run — 4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "Lari 4 km pelan sekali",
            "Tidak usah peduli pace",
            "Kalau capek: lari 5 min, jalan 1 min",
        ],
        "note": "Ini jarak terjauh minggu ini. Jangan tergesa-gesa.",
        "emoji": "🏃",
    },
    "2026-08-11": {
        "day": "Week 2 —  Selasa (11 Agu)",
        "type": "Istirahat",
        "plan": ["Istirahat total", "Boleh jalan kaki santai 15-20 menit kalau mau"],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-08-12": {
        "day": "Week 2 —  Rabu (12 Agu)",
        "type": "Easy Run — 3 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "Lari 3 km santai",
        ],
        "note": "Masih minggu pertama, jangan dipaksa.",
        "emoji": "🏃",
    },

    # Week 2
    "2026-08-13": {
        "day": "Week 2 —  Kamis (13 Agu)",
        "type": "Easy Run — 3.5-4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "Lari 3.5-4 km santai",
        ],
        "note": "Coba bandingkan pace dengan minggu lalu — harusnya mulai ada perbaikan.",
        "emoji": "🏃",
    },
    "2026-08-14": {
        "day": "Week 2 —  Jumat (14 Agu)",
        "type": "Strength Training — 20 min",
        "plan": [
            "2 ronde, tanpa beban:",
            "• 15 Squat",
            "• 10 Lunge tiap kaki",
            "• 15 Glute Bridge",
            "• 20 Calf Raise",
            "• Plank 30-45 detik",
            "Istirahat 60 detik antar ronde",
        ],
        "note": "Tambah 15 detik di plank dari minggu lalu.",
        "emoji": "💪",
    },
    "2026-08-15": {
        "day": "Week 2 —  Sabtu (15 Agu)",
        "type": "Interval Training",
        "plan": [
            "Pemanasan jalan 5 menit",
            "6 kali:",
            "  • Lari 2 menit (agak cepat tapi masih terkendali)",
            "  • Jalan 2 menit",
            "Pendinginan jalan 5 menit",
        ],
        "note": "Ini latihan yang membangun kecepatan dan kapasitas jantung-paru.",
        "emoji": "⚡",
    },
    "2026-08-16": {
        "day": "Week 2 —  Minggu (16 Agu)",
        "type": "Istirahat",
        "plan": ["Istirahat total", "Stretching ringan boleh dilakukan"],
        "note": "Besok long run 5km, simpan energi.",
        "emoji": "😴",
    },
    "2026-08-17": {
        "day": "Week 2 —  Senin (17 Agu)",
        "type": "Long Run — 5-6 KM 🔑",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "Lari 5 km santai",
            "Kalau di KM 5 masih sanggup (RPE <6): lanjutkan pelan sampai 6 km",
            "Bukan ngebut — tujuannya supaya otak tahu '6 km itu bisa'",
            "Strategi: lari 5 min, jalan 1 min kalau perlu",
        ],
        "note": "Ini sesi paling penting! Kalau berhasil menyentuh 6km sekali saja, mental akan jauh lebih siap untuk race day.",
        "emoji": "🔑",
    },
    "2026-08-18": {
        "day": "Race Week —  Selasa (18 Agu)",
        "type": "Recovery Walk",
        "plan": [
            "Jalan kaki santai 30 menit",
            "Bisa sambil jalan-jalan di sekitar rumah",
        ],
        "note": "Active recovery — membantu otot pulih lebih cepat daripada diam total.",
        "emoji": "🚶",
    },
    "2026-08-19": {
        "day": "Race Week —  Rabu (19 Agu)",
        "type": "Istirahat",
        "plan": ["Istirahat total"],
        "note": "Minggu race dimulai besok. Tidur cukup malam ini.",
        "emoji": "😴",
    },

    # Race Week
    "2026-08-20": {
        "day": "Race Week —  Kamis (20 Agu)",
        "type": "Easy Run — 3 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "Lari 3 km santai",
        ],
        "note": "Latihan terakhir yang bermakna. Jangan dipaksa, ini cuma menjaga kaki tetap segar.",
        "emoji": "🏃",
    },
    "2026-08-21": {
        "day": "Race Week —  Jumat (21 Agu)",
        "type": "Shakeout Run — 2 KM",
        "plan": [
            "Lari 2 km sangat santai",
            "Hanya untuk menjaga feel lari, bukan latihan",
        ],
        "note": "2 hari menuju race. Jangan lakukan apapun yang bikin pegal.",
        "emoji": "🏃",
    },
    "2026-08-22": {
        "day": "Race Week —  Sabtu (22 Agu)",
        "type": "Istirahat Total — Persiapan Race",
        "plan": [
            "TIDAK ADA LATIHAN",
            "Siapkan sepatu lari (yang sudah dipakai latihan, BUKAN yang baru)",
            "Siapkan pakaian lari",
            "Perbanyak karbohidrat: nasi, kentang, pasta",
            "Tidur cukup — target 7-8 jam",
        ],
        "note": "Besok race day! 🎉 Pagi ini makan 2-3 jam sebelum start: nasi + telur / roti + selai / pisang. Hindari makanan pedas, gorengan, atau yang belum pernah dicoba.",
        "emoji": "🎯",
    },
    "2026-08-23": {
        "day": "RACE DAY — Minggu, 23 Agu 2026",
        "type": "🏁 RACE 6 KM",
        "plan": [
            "Strategi race:",
            "• KM 0-2: Pelan sekali — harus terasa 'terlalu mudah'",
            "• KM 2-4: Masuk ke ritme yang nyaman",
            "• KM 4-5: Kalau lelah, perlambat daripada berhenti total",
            "• KM 5-6: Kalau masih ada tenaga, gas sampai finish!",
            "",
            "Kalau napas berat: lari 4-5 min, jalan 1 min",
            "Target waktu: 45-60 menit",
        ],
        "note": "Kamu sudah berlatih 18 hari untuk ini. Percaya diri, nikmati lomba, dan finish dengan selamat! 💪🎉",
        "emoji": "🏁",
    },
    # ==== PHASE 1 BASE (ZONE 2) — post-race 25 Agu s/d 20 Sep 2026 ====
    "2026-08-24": {
        "day": "Phase 1 Base —  Senin (24 Aug)",
        "type": "Recovery Run Z2 — 2-3 KM (opsional)",
        "plan": [
            "H+1 race: cek kaki dulu — kalau nyeri berat, ganti jalan cepat 30 menit",
            "Kalau OK: lari 2-3 km SANGAT pelan (9:00-9:30/km), HR <130",
            "Ini bukan latihan — cuma membantu pemulihan",
        ],
        "note": "Recovery run tidak menambah fitness; fungsinya melancarkan darah ke otot. Kalau ragu, jalan saja.",
        "emoji": "🏃",
    },
    "2026-08-25": {
        "day": "Phase 1 Base —  Selasa (25 Aug)",
        "type": "Recovery — Istirahat Total",
        "plan": ["Recovery pasca race", "Tidur cukup, hidrasi, makan protein"],
        "note": "Otot sedang memperbaiki diri. Jangan lari dulu.",
        "emoji": "😴",
    },
    "2026-08-26": {
        "day": "Phase 1 Base —  Rabu (26 Aug)",
        "type": "Recovery Walk — 30 menit",
        "plan": ["Jalan cepat santai 30 menit", "HR harus tetap rendah (<110)"],
        "note": "Peredaran darah membantu pemulihan.",
        "emoji": "🚶",
    },
    "2026-08-27": {
        "day": "Phase 1 Base —  Kamis (27 Aug)",
        "type": "Strength Training — 20 menit",
        "plan": ["2 ronde: 15 squat, 10 lunge/kaki, 15 glute bridge, 20 calf raise, plank 30 dtk", "Istirahat 60 dtk antar ronde"],
        "note": "Form, bukan berat.",
        "emoji": "💪",
    },
    "2026-08-28": {
        "day": "Phase 1 Base —  Jumat (28 Aug)",
        "type": "Istirahat",
        "plan": ["Full rest"],
        "note": "",
        "emoji": "😴",
    },
    "2026-08-29": {
        "day": "Phase 1 Base —  Sabtu (29 Aug)",
        "type": "Zone 2 Run — 3 KM",
        "plan": ["Jalan 5 menit pemanasan", "Lari 3 km di 8:30-9:30/km", "Harus bisa ngobrol penuh — kalau ngos-ngosan, terlalu cepat", "HR target: 114-133 bpm"],
        "note": "Run Z2 pertama! Fokus rasa, bukan angka.",
        "emoji": "🏃",
    },
    "2026-08-30": {
        "day": "Phase 1 Base —  Minggu (30 Aug)",
        "type": "Istirahat Aktif — Stretching",
        "plan": ["Stretching 15-20 menit", "Hamstring, quad, calf, hip flexor"],
        "note": "",
        "emoji": "🧘",
    },
    "2026-08-31": {
        "day": "Phase 1 Base —  Senin (31 Aug)",
        "type": "Zone 2 Run — 4 KM",
        "plan": ["Pemanasan jalan 5 menit", "4 km @ 8:30-9:30/km", "Talk test: kalimat panjang tanpa ngos", "HR 114-133 bpm"],
        "note": "",
        "emoji": "🏃",
    },
    "2026-09-01": {
        "day": "Phase 1 Base —  Selasa (1 Sep)",
        "type": "Istirahat",
        "plan": ["Full rest"],
        "note": "",
        "emoji": "😴",
    },
    "2026-09-02": {
        "day": "Phase 1 Base —  Rabu (2 Sep)",
        "type": "Zone 2 Run — 4 KM",
        "plan": ["Pemanasan jalan 5 menit", "4 km @ 8:30-9:30/km", "Cadence ≥160 spm, langkah pendek"],
        "note": "",
        "emoji": "🏃",
    },
    "2026-09-03": {
        "day": "Phase 1 Base —  Kamis (3 Sep)",
        "type": "Strength Training — 25 menit",
        "plan": ["3 ronde: 15 squat, 12 lunge/kaki, 15 glute bridge, 20 calf raise, plank 45 dtk", "Istirahat 60 dtk antar ronde"],
        "note": "Naikkan volume dikit.",
        "emoji": "💪",
    },
    "2026-09-04": {
        "day": "Phase 1 Base —  Jumat (4 Sep)",
        "type": "Long Run Z2 — 6 KM",
        "plan": ["Jalan 5 menit pemanasan", "6 km pelan — boleh mix lari+jalan cepat", "HR tidak boleh >140"],
        "note": "Long run pertama pasca race!",
        "emoji": "🏃",
    },
    "2026-09-05": {
        "day": "Phase 1 Base —  Sabtu (5 Sep)",
        "type": "Zone 2 Run — 5 KM",
        "plan": ["Pemanasan jalan 5 menit", "5 km @ 8:30-9:30/km", "Z2 discipline: kalau dilipat orang, biarkan"],
        "note": "",
        "emoji": "🏃",
    },
    "2026-09-06": {
        "day": "Phase 1 Base —  Minggu (6 Sep)",
        "type": "Istirahat",
        "plan": ["Full rest"],
        "note": "",
        "emoji": "😴",
    },
    "2026-09-07": {
        "day": "Phase 1 Base —  Senin (7 Sep)",
        "type": "Istirahat",
        "plan": ["Full rest"],
        "note": "",
        "emoji": "😴",
    },
    "2026-09-08": {
        "day": "Phase 1 Base —  Selasa (8 Sep)",
        "type": "Zone 2 Run — 5 KM",
        "plan": ["Pemanasan jalan 5 menit", "5 km @ 8:30-9:30/km", "Fokus cadence 165"],
        "note": "",
        "emoji": "🏃",
    },
    "2026-09-09": {
        "day": "Phase 1 Base —  Rabu (9 Sep)",
        "type": "Interval — 4×3 menit (pengenalan)",
        "plan": [
            "Warm up: jalan 5 menit + lari pelan 10 menit",
            "4×3 MENIT keras (RPE 8, ngobrol mustahil, ~pace 5K) / 2 menit jog pelan",
            "Cool down: lari pelan + jalan 10 menit",
            "Interval PERTAMA — target rata, bukan sprint. Kalau nyeri tajam: STOP",
        ],
        "note": "Ganti strength hari ini; strength pindah ke hari Jumat. Hard day: kemarin & besok harus easy/rest.",
        "emoji": "⚡",
    },
    "2026-09-11": {
        "day": "Phase 1 Base —  Jumat (11 Sep)",
        "type": "Long Run Z2 — 7 KM",
        "plan": ["Jalan 5 menit", "7 km pelan konsisten", "Bawa air"],
        "note": "Volume naik — tetap Z2, tidak boleh ngos.",
        "emoji": "🏃",
    },
    "2026-09-10": {
        "day": "Phase 1 Base —  Kamis (10 Sep)",
        "type": "Zone 2 Recovery Run — 4 KM",
        "plan": [
            "Pemanasan jalan 5 menit",
            "4 km SANGAT pelan (9:00-9:30/km)",
            "Kemarin interval — hari ini wajib easy, cek kaki",
        ],
        "note": "Easy day setelah interval: kalau pegel parah, turun ke jalan cepat 30 menit.",
        "emoji": "🚶",
    },
    "2026-09-12": {
        "day": "Phase 1 Base —  Sabtu (12 Sep)",
        "type": "Zone 2 Run — 6 KM",
        "plan": ["Pemanasan jalan 5 menit", "6 km @ 8:30-9:30/km"],
        "note": "",
        "emoji": "🏃",
    },
    "2026-09-13": {
        "day": "Phase 1 Base —  Minggu (13 Sep)",
        "type": "Strength Training — 25 menit",
        "plan": [
            "3 ronde: 15 squat, 12 lunge/kaki, 15 glute bridge, 20 calf raise, plank 45 dtk",
            "Istirahat 60 dtk antar ronde",
        ],
        "note": "Pindahan dari Rabu (slotnya dipakai interval). Kaki interval bekerja di sini.",
        "emoji": "💪",
    },
    "2026-09-14": {
        "day": "Phase 1 Base —  Senin (14 Sep)",
        "type": "Istirahat",
        "plan": ["Full rest"],
        "note": "",
        "emoji": "😴",
    },
    "2026-09-15": {
        "day": "Phase 1 Base —  Selasa (15 Sep)",
        "type": "Zone 2 Run — 5 KM",
        "plan": ["Pemanasan jalan 5 menit", "5 km @ 8:30-9:30/km"],
        "note": "",
        "emoji": "🏃",
    },
    "2026-09-16": {
        "day": "Phase 1 Base —  Rabu (16 Sep)",
        "type": "Interval — Norwegian 4×4 (full)",
        "plan": [
            "Warm up: jalan 5 menit + lari pelan 10-15 menit",
            "4×4 MENIT keras (RPE 8-9, ~90-95% HRmax) / 3 menit jog pelan",
            "Cool down: lari pelan + jalan 10 menit",
            "Pace keras: konsisten di 4 rep — rep terakhir tidak boleh collapse",
        ],
        "note": "Full 4×4 pertama. Kemarin & besok wajib easy. Kalau masih pegel dari minggu lalu, ulangi 4×3 dulu.",
        "emoji": "⚡",
    },
    "2026-09-17": {
        "day": "Phase 1 Base —  Kamis (17 Sep)",
        "type": "Zone 2 Recovery Run — 4 KM",
        "plan": [
            "Pemanasan jalan 5 menit",
            "4 km SANGAT pelan (9:00-9:30/km)",
            "Kemarin full 4×4 — hari ini wajib easy",
        ],
        "note": "Recovery setelah interval. Pegel parah? Jalan cepat 30 menit saja.",
        "emoji": "🚶",
    },
    "2026-09-18": {
        "day": "Phase 1 Base —  Jumat (18 Sep)",
        "type": "Long Run Z2 — 8 KM",
        "plan": ["Jalan 5 menit", "8 km pelan", "Target Phase 1: mampu 8-10 K"],
        "note": "Graduation run Phase 1! 🎓",
        "emoji": "🏃",
    },
    "2026-09-19": {
        "day": "Phase 1 Base —  Sabtu (19 Sep)",
        "type": "Zone 2 Run — 6 KM",
        "plan": ["Pemanasan jalan 5 menit", "6 km @ 8:30-9:30/km", "Cek: apakah pace Z2 terasa lebih mudah dari minggu 1?"],
        "note": "Progress check: HR turun di pace sama = engine terbangun.",
        "emoji": "🏃",
    },
    "2026-09-20": {
        "day": "Phase 1 Base —  Minggu (20 Sep)",
        "type": "Strength Training — 25 menit",
        "plan": [
            "3 ronde: 15 squat, 12 lunge/kaki, 15 glute bridge, 20 calf raise, plank 45 dtk",
            "Istirahat 60 dtk antar ronde",
        ],
        "note": "Slot strength mingguan (Rabu dipakai interval).",
        "emoji": "💪",
    },
}


def get_today_key():
    return datetime.now(timezone(timedelta(hours=7))).strftime("%Y-%m-%d")


def load_progress():
    if os.path.exists(PROGRESS_FILE):
        with open(PROGRESS_FILE) as f:
            return json.load(f)
    return {}


def save_progress(data):
    os.makedirs(os.path.dirname(PROGRESS_FILE), exist_ok=True)
    with open(PROGRESS_FILE, "w") as f:
        json.dump(data, f, indent=2, ensure_ascii=False)


def format_today():
    """For cron job — returns empty string on non-scheduled days (silent)."""
    today = get_today_key()
    schedule = SCHEDULE.get(today)
    if not schedule:
        return ""

    lines = []
    lines.append(f"{schedule['emoji']} *Latihan Hari Ini — {schedule['day']}*")
    lines.append(f"_{schedule['type']}_\n")
    for item in schedule["plan"]:
        lines.append(f"• {item}")
    lines.append(f"\n💡 {schedule['note']}")
    return "\n".join(lines)


def get_progress_summary():
    progress = load_progress()
    if not progress:
        return None

    completed = [k for k, v in progress.items() if v.get("status") == "done"]
    skipped = [k for k, v in progress.items() if v.get("status") == "skipped"]
    total_scheduled = len(SCHEDULE)

    lines = []
    lines.append(f"📊 *Progress Tracker — 6KM Race Training*\n")
    lines.append(f"Completed: {len(completed)}/{total_scheduled} sessions | Skipped: {len(skipped)}")

    if completed:
        lines.append(f"\n✅ *Sessions completed:*")
        for date_key in sorted(completed):
            s = SCHEDULE.get(date_key, {})
            p = progress[date_key]
            detail_parts = []
            if p.get("distance"):
                detail_parts.append(f"Jarak: {p['distance']} km")
            if p.get("duration"):
                detail_parts.append(f"Durasi: {p['duration']}")
            if p.get("stops") is not None:
                detail_parts.append(f"Berhenti: {p['stops']}x")
            if p.get("pace"):
                detail_parts.append(f"Pace: {p['pace']}/km")
            if p.get("rpe"):
                detail_parts.append(f"RPE: {p['rpe']}/10")
            if p.get("notes"):
                detail_parts.append(p["notes"])
            detail_str = " | ".join(detail_parts)
            lines.append(f"  • {date_key} — {s.get('type', '?')}: {detail_str}")

    if skipped:
        lines.append(f"\n⏭️ *Skipped:*")
        for date_key in sorted(skipped):
            s = SCHEDULE.get(date_key, {})
            lines.append(f"  • {date_key} — {s.get('type', '?')}")

    # Upcoming
    today = get_today_key()
    upcoming = [k for k in sorted(SCHEDULE.keys()) if k >= today and k not in progress]
    if upcoming:
        lines.append(f"\n📅 *Mendatang:*")
        for date_key in upcoming[:5]:
            s = SCHEDULE[date_key]
            lines.append(f"  • {date_key} — {s['type']}")

    return "\n".join(lines)


def log_session(date_key, distance=None, duration=None, stops=None, rpe=None, notes=None):
    progress = load_progress()
    entry = {"status": "done"}
    if distance:
        entry["distance"] = distance
    if duration:
        entry["duration"] = duration
    if stops is not None:
        entry["stops"] = stops
    if rpe is not None:
        entry["rpe"] = rpe
    if notes:
        entry["notes"] = notes
    progress[date_key] = entry
    save_progress(progress)
    return f"✅ Logged: {date_key} — {entry}"


def skip_session(date_key):
    progress = load_progress()
    progress[date_key] = {"status": "skipped"}
    save_progress(progress)
    return f"⏭️ Skipped: {date_key}"


if __name__ == "__main__":
    if len(sys.argv) > 1:
        cmd = sys.argv[1]
        if cmd == "summary":
            result = get_progress_summary()
            print(result or "Belum ada progress yang dicatat.")
        elif cmd == "today":
            result = format_today()
            print(result)  # empty = silent for cron
        elif cmd == "log":
            date_key = sys.argv[2] if len(sys.argv) > 2 else get_today_key()
            distance = sys.argv[3] if len(sys.argv) > 3 else None
            duration = sys.argv[4] if len(sys.argv) > 4 else None
            stops = int(sys.argv[5]) if len(sys.argv) > 5 else None
            rpe = int(sys.argv[6]) if len(sys.argv) > 6 else None
            notes = sys.argv[7] if len(sys.argv) > 7 else None
            print(log_session(date_key, distance, duration, stops, rpe, notes))
        elif cmd == "skip":
            date_key = sys.argv[2] if len(sys.argv) > 2 else get_today_key()
            print(skip_session(date_key))
    else:
        # Default: for cron — print today's reminder (empty if no schedule)
        print(format_today())

# ==== 10K RACE PLAN (6 Des 2026) — auto-generated ====
SCHEDULE.update({
    "2026-09-21": {
        "day": "10K W1 —  Senin (21 Sep)",
        "type": "Easy Run — 4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "4 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-09-22": {
        "day": "10K W1 —  Selasa (22 Sep)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-09-23": {
        "day": "10K W1 —  Rabu (23 Sep)",
        "type": "Easy Run — 4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "4 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-09-24": {
        "day": "10K W1 —  Kamis (24 Sep)",
        "type": "Strength Training — 25 min",
        "plan": [
            "3 ronde, tanpa beban:",
            "• 15 Squat",
            "• 12 Lunge tiap kaki",
            "• 15 Glute Bridge",
            "• 20 Calf Raise",
            "• Plank 45 detik",
            "Istirahat 60 detik antar ronde",
        ],
        "note": "Strength pendukung lari. Fokus form.",
        "emoji": "💪",
    },
    "2026-09-25": {
        "day": "10K W1 —  Sabtu (26 Sep)",
        "type": "Long Run — 7 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "Lari 7 km pelan, pace 8:30-9:30/km",
            "Boleh lari 5 min / jalan 1 min kalau perlu",
            "Jalan 5 menit cooldown",
        ],
        "note": "Long run = fondasi 10K. Pelan sekali, yang penting selesai.",
        "emoji": "🏃",
    },
    "2026-09-26": {
        "day": "10K W1 —  Jumat (25 Sep)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-09-27": {
        "day": "10K W1 —  Minggu (27 Sep)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-09-28": {
        "day": "10K W2 —  Senin (28 Sep)",
        "type": "Easy Run — 4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "4 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-09-29": {
        "day": "10K W2 —  Selasa (29 Sep)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-09-30": {
        "day": "10K W2 —  Rabu (30 Sep)",
        "type": "Easy Run — 4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "4 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-10-01": {
        "day": "10K W2 —  Kamis (1 Oct)",
        "type": "Strength Training — 25 min",
        "plan": [
            "3 ronde, tanpa beban:",
            "• 15 Squat",
            "• 12 Lunge tiap kaki",
            "• 15 Glute Bridge",
            "• 20 Calf Raise",
            "• Plank 45 detik",
            "Istirahat 60 detik antar ronde",
        ],
        "note": "Strength pendukung lari. Fokus form.",
        "emoji": "💪",
    },
    "2026-10-02": {
        "day": "10K W2 —  Sabtu (3 Oct)",
        "type": "Long Run — 8 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "Lari 8 km pelan, pace 8:30-9:30/km",
            "Boleh lari 5 min / jalan 1 min kalau perlu",
            "Jalan 5 menit cooldown",
        ],
        "note": "Long run = fondasi 10K. Pelan sekali, yang penting selesai.",
        "emoji": "🏃",
    },
    "2026-10-03": {
        "day": "10K W2 —  Jumat (2 Oct)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-10-04": {
        "day": "10K W2 —  Minggu (4 Oct)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-10-05": {
        "day": "10K W3 —  Senin (5 Oct)",
        "type": "Easy Run — 4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "4 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-10-06": {
        "day": "10K W3 —  Selasa (6 Oct)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-10-07": {
        "day": "10K W3 —  Rabu (7 Oct)",
        "type": "Easy Run — 4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "4 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-10-08": {
        "day": "10K W3 —  Kamis (8 Oct)",
        "type": "Strength Training — 25 min",
        "plan": [
            "3 ronde, tanpa beban:",
            "• 15 Squat",
            "• 12 Lunge tiap kaki",
            "• 15 Glute Bridge",
            "• 20 Calf Raise",
            "• Plank 45 detik",
            "Istirahat 60 detik antar ronde",
        ],
        "note": "Strength pendukung lari. Fokus form.",
        "emoji": "💪",
    },
    "2026-10-09": {
        "day": "10K W3 —  Sabtu (10 Oct)",
        "type": "Long Run — 8 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "Lari 8 km pelan, pace 8:30-9:30/km",
            "Boleh lari 5 min / jalan 1 min kalau perlu",
            "Jalan 5 menit cooldown",
        ],
        "note": "Long run = fondasi 10K. Pelan sekali, yang penting selesai.",
        "emoji": "🏃",
    },
    "2026-10-10": {
        "day": "10K W3 —  Jumat (9 Oct)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-10-11": {
        "day": "10K W3 —  Minggu (11 Oct)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-10-12": {
        "day": "10K W4 —  Senin (12 Oct)",
        "type": "Easy Run — 4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "4 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-10-13": {
        "day": "10K W4 —  Selasa (13 Oct)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-10-14": {
        "day": "10K W4 —  Rabu (14 Oct)",
        "type": "Easy Run — 4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "4 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-10-REDACTED": {
        "day": "10K W4 —  Kamis (15 Oct)",
        "type": "Strength Training — 25 min",
        "plan": [
            "3 ronde, tanpa beban:",
            "• 15 Squat",
            "• 12 Lunge tiap kaki",
            "• 15 Glute Bridge",
            "• 20 Calf Raise",
            "• Plank 45 detik",
            "Istirahat 60 detik antar ronde",
        ],
        "note": "Strength pendukung lari. Fokus form.",
        "emoji": "💪",
    },
    "2026-10-REDACTED": {
        "day": "10K W4 —  Sabtu (17 Oct)",
        "type": "Long Run — 9 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "Lari 9 km pelan, pace 8:30-9:30/km",
            "Boleh lari 5 min / jalan 1 min kalau perlu",
            "Jalan 5 menit cooldown",
        ],
        "note": "Long run = fondasi 10K. Pelan sekali, yang penting selesai.",
        "emoji": "🏃",
    },
    "2026-10-REDACTED": {
        "day": "10K W4 —  Jumat (16 Oct)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-10-REDACTED": {
        "day": "10K W4 —  Minggu (18 Oct)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-10-19": {
        "day": "10K W5 —  Senin (19 Oct)",
        "type": "Easy Run — 4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "4 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-10-20": {
        "day": "10K W5 —  Selasa (20 Oct)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-10-21": {
        "day": "10K W5 —  Rabu (21 Oct)",
        "type": "Easy Run — 4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "4 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-10-22": {
        "day": "10K W5 —  Kamis (22 Oct)",
        "type": "Strength Training — 25 min",
        "plan": [
            "3 ronde, tanpa beban:",
            "• 15 Squat",
            "• 12 Lunge tiap kaki",
            "• 15 Glute Bridge",
            "• 20 Calf Raise",
            "• Plank 45 detik",
            "Istirahat 60 detik antar ronde",
        ],
        "note": "Strength pendukung lari. Fokus form.",
        "emoji": "💪",
    },
    "2026-10-23": {
        "day": "10K W5 —  Sabtu (24 Oct)",
        "type": "Long Run — 9 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "Lari 9 km pelan, pace 8:30-9:30/km",
            "Boleh lari 5 min / jalan 1 min kalau perlu",
            "Jalan 5 menit cooldown",
        ],
        "note": "Long run = fondasi 10K. Pelan sekali, yang penting selesai.",
        "emoji": "🏃",
    },
    "2026-10-24": {
        "day": "10K W5 —  Jumat (23 Oct)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-10-25": {
        "day": "10K W5 —  Minggu (25 Oct)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-10-26": {
        "day": "10K W6 —  Senin (26 Oct)",
        "type": "Easy Run — 4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "4 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-10-27": {
        "day": "10K W6 —  Selasa (27 Oct)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-10-28": {
        "day": "10K W6 —  Rabu (28 Oct)",
        "type": "Easy Run — 4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "4 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-10-29": {
        "day": "10K W6 —  Kamis (29 Oct)",
        "type": "Strength Training — 25 min",
        "plan": [
            "3 ronde, tanpa beban:",
            "• 15 Squat",
            "• 12 Lunge tiap kaki",
            "• 15 Glute Bridge",
            "• 20 Calf Raise",
            "• Plank 45 detik",
            "Istirahat 60 detik antar ronde",
        ],
        "note": "Strength pendukung lari. Fokus form.",
        "emoji": "💪",
    },
    "2026-10-30": {
        "day": "10K W6 —  Sabtu (31 Oct)",
        "type": "Long Run — 10 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "Lari 10 km pelan, pace 8:30-9:30/km",
            "Boleh lari 5 min / jalan 1 min kalau perlu",
            "Jalan 5 menit cooldown",
        ],
        "note": "Long run = fondasi 10K. Pelan sekali, yang penting selesai.",
        "emoji": "🏃",
    },
    "2026-10-31": {
        "day": "10K W6 —  Jumat (30 Oct)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-11-01": {
        "day": "10K W6 —  Minggu (1 Nov)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-11-02": {
        "day": "10K W7 —  Senin (2 Nov)",
        "type": "Easy Run — 4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "4 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-11-03": {
        "day": "10K W7 —  Selasa (3 Nov)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-11-04": {
        "day": "10K W7 —  Rabu (4 Nov)",
        "type": "Easy Run — 4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "4 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-11-05": {
        "day": "10K W7 —  Kamis (5 Nov)",
        "type": "Strength Training — 25 min",
        "plan": [
            "3 ronde, tanpa beban:",
            "• 15 Squat",
            "• 12 Lunge tiap kaki",
            "• 15 Glute Bridge",
            "• 20 Calf Raise",
            "• Plank 45 detik",
            "Istirahat 60 detik antar ronde",
        ],
        "note": "Strength pendukung lari. Fokus form.",
        "emoji": "💪",
    },
    "2026-11-06": {
        "day": "10K W7 —  Sabtu (7 Nov)",
        "type": "Long Run — 10 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "Lari 10 km pelan, pace 8:30-9:30/km",
            "Boleh lari 5 min / jalan 1 min kalau perlu",
            "Jalan 5 menit cooldown",
        ],
        "note": "Long run = fondasi 10K. Pelan sekali, yang penting selesai.",
        "emoji": "🏃",
    },
    "2026-11-07": {
        "day": "10K W7 —  Jumat (6 Nov)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-11-08": {
        "day": "10K W7 —  Minggu (8 Nov)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-11-09": {
        "day": "10K W8 —  Senin (9 Nov)",
        "type": "Easy Run — 4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "4 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-11-10": {
        "day": "10K W8 —  Selasa (10 Nov)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-11-11": {
        "day": "10K W8 —  Rabu (11 Nov)",
        "type": "Easy Run — 4 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "4 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-11-12": {
        "day": "10K W8 —  Kamis (12 Nov)",
        "type": "Strength Training — 25 min",
        "plan": [
            "3 ronde, tanpa beban:",
            "• 15 Squat",
            "• 12 Lunge tiap kaki",
            "• 15 Glute Bridge",
            "• 20 Calf Raise",
            "• Plank 45 detik",
            "Istirahat 60 detik antar ronde",
        ],
        "note": "Strength pendukung lari. Fokus form.",
        "emoji": "💪",
    },
    "2026-11-13": {
        "day": "10K W8 —  Sabtu (14 Nov)",
        "type": "Long Run — 10 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "Lari 10 km pelan, pace 8:30-9:30/km",
            "Boleh lari 5 min / jalan 1 min kalau perlu",
            "Jalan 5 menit cooldown",
        ],
        "note": "Long run = fondasi 10K. Pelan sekali, yang penting selesai.",
        "emoji": "🏃",
    },
    "2026-11-14": {
        "day": "10K W8 —  Jumat (13 Nov)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-11-15": {
        "day": "10K W8 —  Minggu (15 Nov)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-11-16": {
        "day": "10K W9 —  Senin (16 Nov)",
        "type": "Easy Run — 5 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "5 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-11-17": {
        "day": "10K W9 —  Selasa (17 Nov)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-11-18": {
        "day": "10K W9 —  Rabu (18 Nov)",
        "type": "Easy Run — 5 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "5 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-11-19": {
        "day": "10K W9 —  Kamis (19 Nov)",
        "type": "Strength Training — 25 min",
        "plan": [
            "3 ronde, tanpa beban:",
            "• 15 Squat",
            "• 12 Lunge tiap kaki",
            "• 15 Glute Bridge",
            "• 20 Calf Raise",
            "• Plank 45 detik",
            "Istirahat 60 detik antar ronde",
        ],
        "note": "Strength pendukung lari. Fokus form.",
        "emoji": "💪",
    },
    "2026-11-20": {
        "day": "10K W9 —  Sabtu (21 Nov)",
        "type": "Long Run — 9 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "Lari 9 km pelan, pace 8:30-9:30/km",
            "Boleh lari 5 min / jalan 1 min kalau perlu",
            "Jalan 5 menit cooldown",
        ],
        "note": "Long run = fondasi 10K. Pelan sekali, yang penting selesai.",
        "emoji": "🏃",
    },
    "2026-11-21": {
        "day": "10K W9 —  Jumat (20 Nov)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-11-22": {
        "day": "10K W9 —  Minggu (22 Nov)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-11-23": {
        "day": "10K W10 —  Senin (23 Nov)",
        "type": "Easy Run — 5 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "5 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-11-24": {
        "day": "10K W10 —  Selasa (24 Nov)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-11-25": {
        "day": "10K W10 —  Rabu (25 Nov)",
        "type": "Easy Run — 5 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "5 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener.",
        "emoji": "🏃",
    },
    "2026-11-26": {
        "day": "10K W10 —  Kamis (26 Nov)",
        "type": "Strength Training — 25 min",
        "plan": [
            "3 ronde, tanpa beban:",
            "• 15 Squat",
            "• 12 Lunge tiap kaki",
            "• 15 Glute Bridge",
            "• 20 Calf Raise",
            "• Plank 45 detik",
            "Istirahat 60 detik antar ronde",
        ],
        "note": "Strength pendukung lari. Fokus form.",
        "emoji": "💪",
    },
    "2026-11-27": {
        "day": "10K W10 —  Sabtu (28 Nov)",
        "type": "Long Run — 6 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "Lari 6 km pelan, pace 8:30-9:30/km",
            "Boleh lari 5 min / jalan 1 min kalau perlu",
            "Jalan 5 menit cooldown",
        ],
        "note": "Long run = fondasi 10K. Pelan sekali, yang penting selesai.",
        "emoji": "🏃",
    },
    "2026-11-28": {
        "day": "10K W10 —  Jumat (27 Nov)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-11-29": {
        "day": "10K W10 —  Minggu (29 Nov)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-11-30": {
        "day": "10K W11 —  Senin (30 Nov)",
        "type": "Easy Run — 3 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "3 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener. Mantain feel, jangan nambah di race week.",
        "emoji": "🏃",
    },
    "2026-12-01": {
        "day": "10K W11 —  Selasa (1 Dec)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-12-02": {
        "day": "10K W11 —  Rabu (2 Dec)",
        "type": "Easy Run — 3 KM",
        "plan": [
            "Jalan 5 menit (pemanasan)",
            "3 km @ 8:30-9:30/km",
            "Cadence ≥160 spm, langkah pendek",
        ],
        "note": "Bisa ngobrol sambil lari = pace bener. Mantain feel, jangan nambah di race week.",
        "emoji": "🏃",
    },
    "2026-12-03": {
        "day": "10K W11 —  Kamis (3 Dec)",
        "type": "Rest / Mobility ringan",
        "plan": [
            "Stretching & mobility 10 menit",
            "Tidur cukup — recovery pre-race",
        ],
        "note": "Race week: jangan latihan berat. Simpan energi.",
        "emoji": "😴",
    },
    "2026-12-04": {
        "day": "10K W11 —  Jumat (4 Dec)",
        "type": "Istirahat",
        "plan": [
            "Istirahat total",
            "Boleh jalan kaki santai 15-20 menit kalau mau",
        ],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    },
    "2026-12-05": {
        "day": "10K W11 —  Sabtu (5 Dec)",
        "type": "Istirahat (pre-race)",
        "plan": [
            "Istirahat total",
            "Siapkan outfit & perlengkapan race besok",
            "Tidur awal",
        ],
        "note": "Sehari sebelum race. Jangan eksperimen apa pun.",
        "emoji": "😴",
    },
    "2026-12-06": {
        "day": "10K W11 —  Minggu (6 Dec)",
        "type": "RACE DAY — 10K 🏁",
        "plan": [
            "Bangun pagi, sarapan ringan 2 jam sebelum start",
            "KM 0-2: mulai SANGAT pelan (terlalu mudah itu benar)",
            "KM 2-8: settle ke rhythm nyaman (8:00-8:30/km)",
            "KM 8-10: sisa energi, finish kuat",
        ],
        "note": "Race 10K! Jangan kejar pace orang lain. Nikmati, itu hasil 14 minggu.",
        "emoji": "🏁",
    },
})
# ==== END 10K RACE PLAN ====
