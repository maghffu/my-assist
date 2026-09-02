# -*- coding: utf-8 -*-
"""Generate & append SCHEDULE entries: 21 Sep - 6 Des 2026 (10K race 6 Des).
Idempotent: strip blok lama sebelum append. Jalankan sekali dari repo root."""
import re
from datetime import date, timedelta

PATH = "/opt/hermes-lite/scripts/running_schedule.py"
HARI = {0: "Senin", 1: "Selasa", 2: "Rabu", 3: "Kamis", 4: "Jumat", 5: "Sabtu", 6: "Minggu"}

START = date(2026, 9, 21)   # Senin
RACE = date(2026, 12, 6)    # Minggu

LONG = {  # Sabtu progression (lanjut dari 8km di 20 Sep, +cutback tiap 3 minggu)
    date(2026, 9, 26): 7, date(2026, 10, 3): 8, date(2026, 10, 10): 8,
    date(2026, 10, 17): 9, date(2026, 10, 24): 9, date(2026, 10, 31): 10,
    date(2026, 11, 7): 10, date(2026, 11, 14): 10, date(2026, 11, 21): 9,
    date(2026, 11, 28): 6,  # taper
}

def week_of(d):
    return (d - START).days // 7 + 1

def entry(d):
    w = week_of(d)
    dow = d.weekday()
    label = f"10K W{w} — {HARI[dow]} ({d.strftime('%d %b')})"
    if d == RACE:
        return {
            "day": label,
            "type": "RACE DAY — 10K 🏁",
            "plan": [
                "Bangun pagi, sarapan ringan 2 jam sebelum start",
                "KM 0-2: mulai SANGAT pelan (terlalu mudah itu benar)",
                "KM 2-8: settle ke rhythm nyaman (8:00-8:30/km)",
                "KM 8-10: sisa energi, finish kuat",
            ],
            "note": "Race 10K! Jangan kejar pace orang lain. Nikmati, itu hasil 14 minggu.",
            "emoji": "🏁",
        }
    if dow == 5:  # Sabtu long run
        km = LONG.get(d)
        if km is None:  # Sabtu race week = istirahat pre-race
            return {
                "day": label,
                "type": "Istirahat (pre-race)",
                "plan": ["Istirahat total", "Siapkan outfit & perlengkapan race besok", "Tidur awal"],
                "note": "Sehari sebelum race. Jangan eksperimen apa pun.",
                "emoji": "😴",
            }
        return {
            "day": label,
            "type": f"Long Run — {km} KM",
            "plan": [
                "Jalan 5 menit (pemanasan)",
                f"Lari {km} km pelan, pace 8:30-9:30/km",
                "Boleh lari 5 min / jalan 1 min kalau perlu",
                "Jalan 5 menit cooldown",
            ],
            "note": "Long run = fondasi 10K. Pelan sekali, yang penting selesai.",
            "emoji": "🏃",
        }
    if dow == 0 or dow == 2:  # Senin/Rabu easy
        km = 4 if w <= 8 else 5
        if w == 11:  # race week
            km = 3
        return {
            "day": label,
            "type": f"Easy Run — {km} KM",
            "plan": [
                "Jalan 5 menit (pemanasan)",
                f"{km} km @ 8:30-9:30/km",
                "Cadence ≥160 spm, langkah pendek",
            ],
            "note": "Bisa ngobrol sambil lari = pace bener." + (" Mantain feel, jangan nambah di race week." if w == 11 else ""),
            "emoji": "🏃",
        }
    if dow == 3:  # Kamis strength
        if w == 11:
            return {
                "day": label,
                "type": "Rest / Mobility ringan",
                "plan": ["Stretching & mobility 10 menit", "Tidur cukup — recovery pre-race"],
                "note": "Race week: jangan latihan berat. Simpan energi.",
                "emoji": "😴",
            }
        return {
            "day": label,
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
        }
    # Selasa, Jumat, Minggu: rest
    return {
        "day": label,
        "type": "Istirahat",
        "plan": ["Istirahat total", "Boleh jalan kaki santai 15-20 menit kalau mau"],
        "note": "Recovery adalah bagian dari latihan.",
        "emoji": "😴",
    }

def py(d):
    e = entry(d)
    plan = ",\n".join(f'            "{p}"' for p in e["plan"])
    return (f'    "{d.isoformat()}": {{\n'
            f'        "day": "{e["day"]}",\n'
            f'        "type": "{e["type"]}",\n'
            f'        "plan": [\n{plan},\n        ],\n'
            f'        "note": "{e["note"]}",\n'
            f'        "emoji": "{e["emoji"]}",\n'
            f'    }},')

block = "\n# ==== 10K RACE PLAN (6 Des 2026) — auto-generated ====\nSCHEDULE.update({\n"
d = START
while d <= RACE:
    block += py(d) + "\n"
    d += timedelta(days=1)
block += "})\n# ==== END 10K RACE PLAN ====\n"

src = open(PATH).read()
src = re.sub(r"\n# ==== 10K RACE PLAN.*?# ==== END 10K RACE PLAN ====\n", "\n", src, flags=re.S)
open(PATH, "w").write(src.rstrip() + "\n" + block)
print("appended entries:", len(SCHEDULE := []) or sum(1 for _ in range(1)) and "")
