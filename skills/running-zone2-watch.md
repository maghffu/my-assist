# Zone 2 Base Training, VO2max & Huawei Watch — rationale coaching (migrasi dari hermes-agent 29 Agu 2026)

Knowledge coaching di balik plan Phase 1 Base (24 Agu – 20 Sep 2026). User engaged dalam topik ini (tanya apa itu zone 2 setelah lihat reel @ironeddy_, minta plan Z2, lalu minta kombinasi volume Z2 + interval 4×4 utk VO2max).

## Apa itu zone 2
- ~60–70% HRmax. User ini (l. 1994): HRmax ≈ 185–190 → Z2 ≈ **111–133 bpm**. Data jamnya: easy run avg HR 118-136 — yang 136 itu kurang tidur, bukan kecepatan.
- Identifikasi tanpa lab: **talk test** (bisa kalimat penuh), napas hidung masih mungkin, kerasa "terlalu pelan", sanggup berjam-jam. Jalan cepat pun bisa Z2 dan tetap dihitung.
- Adaptasi: mitochondria + kapiler density, oksidasi lemak, biaya recovery rendah → memungkinkan volume tinggi tanpa cedera/overtraining.

## Jebakan zone 3
Semua latihan moderat (RPE 6-7, "comfortably hard") terlalu berat untuk recovery dan terlalu mudah untuk memicu adaptasi → stagnasi. Distribusi baik itu **polarized 80/20**: ~80% volume mingguan easy (Z2), ~20% keras (interval/threshold).

## Driver VO2max
1. **Interval intensitas tinggi** — stimulans terbesar. Norwegian 4×4: 10-15 menit WU, 4×4 menit @ RPE 8-9 (~90-95% HRmax, ~pace race 5K, ngobrol mustahil) / 3 menit jog, 10 menit CD. Varian: 4×3, 5×3, 6×800m, hill repeats 8-10×30s.
2. **Volume Z2** — fondasi yang membuat interval bisa terserap (dan melindungi tendon).
3. **Threshold/tempo** (RPE 7, kalimat 3-5 kata) 1×/minggu — menaikkan lactate threshold.
4. Pendukung: penurunan berat badan (VO2max relatif), leg strength, tidur 7-8 jam, konsistensi.

## Progression rules untuk user ini (disetujui 22 Agu 2026)
- 2 minggu pure Z2 **sebelum** interval apapun (training age baru ~3 minggu di awal plan + riwayat overstride → risiko tendon, bukan jantung).
- Sesi interval **menggantikan** satu sesi Z2 — total sesi/minggu tetap, rasio tetap ~80/20.
- Intro 4×3 menit (minggu 3) → full 4×4 (minggu 4). "Hard" = RPE 8, pace merata, BUKAN sprint; rep terakhir tidak boleh kolaps.
- **Hard-easy rule:** hari interval diapit hari easy/rest (di plan: Sel rest sebelum, Kam recovery run sesudah).
- Cek pagi setelahnya: resting HR tinggi / nyeri berat / tidur buruk → downgrade ke Z2, geser interval.

## Indikator engine-building
Di pace yang sama, avg HR turun minggu ke minggu (kelihatan di log screenshot Huawei). Sesi Sab minggu-4 membawa ini sebagai self-check eksplisit.

## Phase 3 (Speed) — slot hill training (permintaan user, 24 Agu 2026)
Wilayah user (REDACTED-CITY) PUNYA bukit beneran — rute ke selatan (arah Doro / Petungkriyono) punya tanjakan berkelanjutan. Opsi saat Phase 3 (~bulan 4-6) tiba, urutan prioritas:
- **Hill repeats asli** di tanjakan sepi: 8-10×30-60 detik keras naik / jog-jalan turun
- **Rute Z2 berbukit** sebagai variasi long run (naik santai, awasi HR drift)
- Fallback: tangga overpass/stadion, treadmill incline 8-12%
Bentuk sesi: hill repeats MENGGANTIKAN satu slot interval (jaga 80/20), aturan hard-easy sandwich sama. Cadence tinggi, langkah pendek, lean dari pergelangan kaki bukan pinggul; turun = langkah kecil, tanpa braking. Catatan: rute bukit butuh motor/mobil sebentar dari rumah — hitung travel waktu saat planning sesi.
**Saat Phase 2 berakhir, tanya user bukit/rute mana yang paling praktis (jarak dari rumah, keamanan lalu lintas, permukaan), lalu bangun weekly plan Phase 3 di sekitarnya (minggu selang-seling: interval datar / hill repeats).**

---

# Huawei Watch Fit 4 — setup alert HR zone (terkonfigurasi 27 Agu 2026)

## Di mana setting-nya

Setting alarm detak jantung saat workout ada di **aplikasi Huawei Health di HP**,
BUKAN di jam:

```
Huawei Health app → Me (Saya) → Settings → Workout settings
→ Exercise heart rate settings → aktifkan "High heart rate"
→ Heart rate limit → set nilai → OK
```

Saat workout, jam bergetar + tampil alert kalau HR lewat batas itu beberapa detik.

## Konfigurasi user

- Jam: **Huawei Watch Fit 4**
- Limit: **133 bpm** — batas atas Zone 2 user (Z2 ≈ 111–133 bpm)
- Framing coaching ke user: setiap getaran = cue untuk memperpendek langkah
  (fix overstride), bukan "kamu tidak fit".

## Jangan tertukar dua alert HR

| Path setting | Fungsi |
|---|---|
| `Me → Settings → Workout settings → Exercise heart rate settings` | **Saat workout** — yang dipakai user |
| `Devices → [jam] → Continuous heart rate monitoring → High heart rate alert` | Monitoring seharian/istirahat, bukan workout |

## Catatan praktis

- Alert hanya aktif selama sesi workout berjalan di jam.
- Kalau audio reminder dimatikan utk tipe workout, alert datang sebagai getaran saja.
- Ceiling Z2 user diturunkan dari HRmax ≈ 186 (usia 32) — lihat bagian zone 2 di atas utk perhitungan lengkap.
