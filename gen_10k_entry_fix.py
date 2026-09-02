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
