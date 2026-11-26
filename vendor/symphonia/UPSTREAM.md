# symphonia snapshot

Repository: https://github.com/joshuarli/play
Base: db7f14bf5b4663c2e5b330643f629edb66e74234 (play `main`), path `vendor/symphonia/symphonia-core`.
Commit: db7f14bf5b4663c2e5b330643f629edb66e74234; no play change was needed, so nothing was pushed.
Origin: play vendors the owner's Symphonia fork, https://github.com/joshuarli/symphonia at `b6336209bb40d57e4ed71079cf7f7fc6c4626aae` (workspace version 0.6.1, derived from pdeljanov/Symphonia), as recorded in play's `vendor/symphonia/UPSTREAM.md`.

The snapshot is the `symphonia-core` package only. Its sources and license are byte-identical to the base. The manifest stands outside play's Symphonia workspace: the inherited workspace fields and dependency versions are written out, the `[lints] workspace = true` table is removed, and the `rand` dev-dependency is dropped. Features and the optional `rustfft` dependency are unchanged; no feature is enabled, so `rustfft` is not in the lockfile.

The media-demux box uses `checksum::Crc8Ccitt` (polynomial 0x07, FFmpeg's `AV_CRC_8_ATM`) for FLAC frame headers and `checksum::Crc16Ansi` (polynomial 0x8005, FFmpeg's `AV_CRC_16_ANSI`) for FLAC frame footers.
