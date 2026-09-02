# 6KM Race Training — Schedule & Progress Tracker (migrasi dari hermes-agent 29 Agu 2026)

Kelola jadwal training & log progress untuk race 6KM.
Triggers: jadwal lari, jadwal running, latihan hari ini, workout hari ini, log lari, progress lari, ganti/switch jadwal.

## Files

| File | Fungsi |
|---|---|
| `/opt/hermes-lite/scripts/running_schedule.py` | Data SCHEDULE (dict keyed by date) + fungsi progress |
| `/opt/hermes-lite/scripts/running_reminder.sh` | Wrapper bash (panggil `running_schedule.py today`) |
| `/opt/hermes-lite/scripts/ocr_workout.py` | Helper OCR screenshot Huawei Health |
| `/opt/hermes-lite/data/running_progress.json` | Log progress (keyed by date) |

## Reminder harian

- Job di tabel `reminders` hermes-lite: **kind=job, recur=daily, 07:50 WIB** — prompt job menjalankan `python3 /opt/hermes-lite/scripts/running_schedule.py today` via run_command lalu mengirim hasilnya.
- **Silent di hari rest:** `format_today()` return string kosong kalau tidak ada entry — tidak ada yang dikirim.

## Race Info

- **RACE BERIKUTNYA:** 6 Desember 2026 (Minggu) — **10K**, plan lengkap 21 Sep–6 Des sudah di SCHEDULE (blok "10K RACE PLAN", long run Sabtu: 7→8→9→10 km, taper 28 Nov + race week).
- **Race lama:** 23 Agustus 2026, 6 KM — 42:59, 6:45/km (SELESAI)
- **Distance:** 6 KM — hasil: 42:59, 6:45/km
- **Training start:** 6 Agustus 2026

## ⚠️ CRITICAL: Consistency Verification Rule

**Setelah SEMUA edit ke SCHEDULE dict — tambah/swap/ubah — WAJIB verifikasi SEMUA entry SEBELUM membalas user.**

### Pola bug-nya

Saat user minta tukar hari (mis. "Senin=Run, Selasa=Rest"), edit parsial bikin mismatch berantai:

1. Agent ganti **activity** di sebuah date key tapi lupa update **label**
2. Agent ganti **label** tapi tidak cek cocok dengan day-of-week tanggal itu
3. Agent ganti activity dua tanggal tapi **arah swap terbalik**

Hasil: user dapat info workout salah, kehilangan trust ke sistem reminder.

### Script verifikasi wajib (jalankan setelah TIAP edit)

```python
from datetime import datetime

hari_id = {'Monday':'Senin','Tuesday':'Selasa','Wednesday':'Rabu',
           'Thursday':'Kamis','Friday':'Jumat','Saturday':'Sabtu','Sunday':'Minggu'}

for d in sorted(SCHEDULE.keys()):
    dt = datetime.strptime(d, '%Y-%m-%d')
    actual = hari_id[dt.strftime('%A')]
    entry = SCHEDULE[d]
    label = entry['day']
    label_day = None
    for h in hari_id.values():
        if h in label:
            label_day = h
            break
    match = '✅' if label_day == actual else f'❌ label={label_day} actual={actual}'
    print(f'{d}: {match} — {entry["emoji"]} {entry["type"]}')
```

Kalau ADA entry ❌, perbaiki DULU sebelum bilang edit selesai.

### Rules

1. **Date key adalah kebenaran yang tetap.** `2026-08-11` selamanya Selasa. Label & activity menyesuaikan tanggal, bukan sebaliknya.
2. **Cron membaca by date key** (`datetime.now().strftime("%Y-%m-%d")`), jadi label cuma kosmetik — ACTIVITY di tanggal itulah yang menentukan.
3. **Saat swap dua hari: tukar ISI activity (type, plan, note, emoji), BUKAN date key atau label.** Date key tetap.
4. **Jangan menebak hari "today" dari konteks obrolan.** Selalu cek `date` / `datetime.now()`.

## Membaca jadwal

### Workout hari ini

```python
import sys; sys.path.insert(0, '/opt/hermes-lite/scripts')
from running_schedule import format_today, get_today_key
from datetime import datetime
today = get_today_key()
dt = datetime.strptime(today, '%Y-%m-%d')
print(f'Tanggal server: {today} ({dt.strftime("%A")})')
print()
print(format_today())
```

### View jadwal lengkap

```python
import sys; sys.path.insert(0, '/opt/hermes-lite/scripts')
from running_schedule import SCHEDULE
for d in sorted(SCHEDULE.keys()):
    s = SCHEDULE[d]
    print(f'{d} — {s["day"]:40s} {s["emoji"]} {s["type"]}')
```

## Log Progress

Progress disimpan di `/opt/hermes-lite/data/running_progress.json`, keyed by date string.

### Log sesi (Python)

```python
import json

with open('/opt/hermes-lite/data/running_progress.json') as f:
    data = json.load(f)

data['2026-08-12'] = {
    'status': 'done',
    'type': 'easy_run',
    'distance': 5.04,
    'duration': '54:33',
    'hr_avg': 118,
    'hr_max': 142,
    'pace_avg': '10:49',
    'cadence': 132,
    'rpe': 6,
    'notes': 'Santai banget'
}

with open('/opt/hermes-lite/data/running_progress.json', 'w') as f:
    json.dump(data, f, indent=2)
```

### Data yang user kasih

User kirim screenshot Huawei Health. CATATAN: di hermes-lite, foto yang dikirim ke bot OTOMATIS di-OCR gateway (Tesseract, Pilar 7) dan teksnya masuk sebagai prompt — baca dari sana. Kalau perlu OCR manual/ulang: helper `/opt/hermes-lite/scripts/ocr_workout.py <image>` via run_command (size check → raw OCR → preprocessing fallback; exit 2 = gambar kekecilan, minta user kirim ulang).

Field yang ditangkap kalau ada:

| Field | Sumber | Contoh |
|---|---|---|
| distance | Ringkasan | 5.04 km |
| duration | Ringkasan | 54:33 |
| pace_avg | Ringkasan | 10:49/km |
| pace_best | Ringkasan | 7:46/km |
| hr_avg | Heart Rate | 118 bpm |
| hr_max | Heart Rate | 142 bpm |
| calories | Ringkasan | 288 kkal |
| cadence | Running Dynamics | 132 spm |
| balance_left/right | Running Dynamics | 49.8% / 50.2% |
| ground_contact | Running Dynamics | 232 ms |
| training_effect | Performance | 1.2 |
| rpe | Laporan user | 6/10 |
| segments | Tabel Segments (splits per-km) | lihat bawah |

**Tabel Segments = paling berharga:** Duration, Distance, Pace, HR avg, Cadence, Stride (cm), Ascent per km. Simpan sebagai list `segments` di entry progress — cadence per-segment menunjukkan user kena 160+ spm di bagian cepat (indikator fix overstride).

Screenshot dark-theme Huawei biasanya OCR bersih RAW: `tesseract <img> stdout -l eng --psm 4` (pernah ekstrak tabel Segments penuh verbatim). Kalau raw jelek: grayscale → invert (dark theme) → upscale 3-6× LANCZOS → contrast ~2.0 → binarize; retry PSM 4/6/11.

**Cek dimensi gambar dulu.** Screenshot rusak/kecil (width < ~200 px): TIDAK ada OCR yang bisa menyelamatkan — minta user kirim ulang atau ketik angka kuncinya. Jangan pernah menebak atau mencatat hasil baca parsial sebagai fakta.

### RPE (Rate of Perceived Exertion)

User self-report RPE skala 1-10:
- 1-3: Sangat mudah, recovery
- 4-5: Mudah, bisa ngobrol penuh
- 6: Moderat, ngobrol mulai berat
- 7-8: Berat, napas berat
- 9-10: Effort maksimal

Easy run target RPE 5-6. RPE 8+ di hari "easy" = overexertion (kemungkinan overstriding).

## Coaching Notes

### Masalah overstride (masalah form utama user)

User cenderung overstride (langkah kejauhan). Gejala:
- RPE tinggi (8+) relatif terhadap pace
- Cadence rendah (~130 spm vs target 160+)
- Pace tidak konsisten (burst cepat, lalu jalan)

**Insight kunci untuk user:** langkah pendek + cadence tinggi = LEBIH efisien, bukan lebih lambat. Pace = Cadence × Panjang Langkah. Overstride menciptakan braking force. Fokus ke cadence, bukan pace.

**Bukti:**
- **15 Agu 2026 interval:** di segmen cepat user natural 160-164 spm dengan stride 70-81 cm, RPE cuma 5 — vs "easy" run sebelumnya RPE 8 di 130 spm. Kesimpulan: masalah effort-nya mekanik stride, bukan fitness.
- **17 Agu 2026 Long Run (6.03 km, 1:00:14):** cadence rata-rata **165** sepanjang 6 km (puncak 175), HR avg 136 — tapi user report RPE 8 karena kurang tidur (tidur 00:30). Pelajaran: **RPE ≠ performa aktual.** Kalau RPE tinggi tapi data HR/cadence bagus, bottleneck-nya recovery/istirahat, bukan fitness.

### Sleep deprivation vs perceived effort

User lapor RPE tinggi padahal metrik objektif solid (HR rendah, cadence tinggi, pace konsisten) → tanya kualitas tidur dulu sebelum menyimpulkan perlu training lebih. Kasus nyata: RPE 8 + HR 136 di 6 km cadence 165 — lelahnya karena begadang, bukan larinya. Tenangkan: dengan tidur cukup pre-race + adrenalin, performa akan jauh lebih baik.

### Sesi yang terlewat

User lapor workout kelewat (sibuk kerja dsb): log di progress file dengan `status: 'skipped'` + `reason` — jaga kejujuran data streak. Lalu tenangkan: skip 1-2 hari di tengah plan dampaknya kecil; alihkan fokus ke sesi KUNCI tersisa (long run pre-race) daripada mengganti yang terlewat.

### Struktur jadwal & swap

User bisa minta swap (mis. pindah rest day). Saat itu:
1. Tukar ISI activity antar dua date key
2. Update label agar cocok hari aktual
3. **Jalankan script verifikasi**
4. Tunjukkan jadwal terkoreksi ke user

Perubahan terakhir (Agu 2026): Senin=Easy Run, Selasa=Istirahat, Rabu=Easy Run (dari back-to-back runs) — isi activity dipindah antar date key. Selalu verifikasi tanggal mana punya activity apa sebelum menjawab "workout hari ini apa?"

## Post-Race Phase (setelah 23 Agu 2026 — sudah lewat)

### Strategi race (utk race berikutnya)

- **KM 0-2:** mulai SANGAT pelan — harusnya kerasa "terlalu mudah" (adrenalin bikin orang kecepatan)
- **KM 2-4:** settle ke rhythm nyaman
- **KM 5-6:** finish kuat
- Jangan jalan — kalau lelah, slow jog (9'/km)
- Pace realistis: 7'30-8'00"/km. JANGAN mulai 5'/km — itu pace 6K 30 menit, buat runner berpengalaman saja.

### Target jangka panjang: pace 5'/km

| Phase | Durasi | Target pace | Fokus |
|---|---|---|---|
| 1 — Base | 1-2 bulan | 8-9'/km | 3x/minggu konsisten, bangun ke 8-10K |
| 2 — Build | 2-3 bulan | 7-8'/km | Volume naik, tempo runs, target race 10K |
| 3 — Speed | 2-3 bulan | 6-7'/km | Interval rutin, hill sprints |
| 4 — Peak | 2-3 bulan | 5-5'30"/km | Threshold runs, target 5K sub-25 menit |
| 5 — Solid | ongoing | **5'/km** | Maintain, race 10K/HM |

Aturan: jangan skip fase (risiko cedera), konsistensi > intensitas, cadence ≥160, race tiap 2-3 bulan sebagai checkpoint. Detail rationale Z2 & hill Phase 3: lihat skill `running-zone2-watch`.

### Transisi skill post-race

Skill ini sudah bergeser dari "race countdown" ke "ongoing training tracker". SCHEDULE perlu diganti weekly plan terstruktur (easy run / interval / long run / rest rotation) — update SCHEDULE dict + Race Info section saat user minta plan baru.
