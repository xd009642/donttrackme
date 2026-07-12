#!/usr/bin/env python3
"""Download Iowa MIS instruments and convert their audio to sampler-ready WAV."""

import argparse
import json
import re
import shutil
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from html.parser import HTMLParser
from pathlib import Path

COLLECTION_URL = "https://theremin.music.uiowa.edu/MIS.html"
USER_AGENT = "donttrackme-sample-fetcher/1.0"
NOTE_RANGE = re.compile(r"^([A-G](?:b|#)?-?\d+)([A-G](?:b|#)?-?\d+)$")


class Links(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.links: list[tuple[str, str]] = []
        self._href: str | None = None
        self._text: list[str] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag == "a":
            self._href = dict(attrs).get("href")
            self._text = []

    def handle_data(self, data: str) -> None:
        if self._href is not None:
            self._text.append(data)

    def handle_endtag(self, tag: str) -> None:
        if tag == "a" and self._href is not None:
            self.links.append((self._href, " ".join(self._text).strip()))
            self._href = None
            self._text = []


def fetch(url: str) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request) as response:
        return response.read()


def page_links(url: str) -> list[tuple[str, str]]:
    parser = Links()
    parser.feed(fetch(url).decode("utf-8", errors="replace"))
    links = []
    for href, text in parser.links:
        absolute = urllib.parse.urlsplit(urllib.parse.urljoin(url, href))
        encoded = urllib.parse.urlunsplit(
            (
                absolute.scheme,
                absolute.netloc,
                urllib.parse.quote(absolute.path, safe="/%"),
                absolute.query,
                absolute.fragment,
            )
        )
        links.append((encoded, text))
    return links


def instrument_pages() -> dict[str, str]:
    pages: dict[str, str] = {}
    for url, _ in page_links(COLLECTION_URL):
        stem = Path(urllib.parse.urlparse(url).path).stem
        if stem.startswith("MIS") and stem != "MIS":
            pages.setdefault(stem.removeprefix("MIS").lower(), url)
    return pages


def download(url: str, destination: Path) -> bool:
    if destination.exists():
        print(f"Using cached {destination.name}")
        return True
    print(f"Downloading {url}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_suffix(destination.suffix + ".part")
    try:
        temporary.write_bytes(fetch(url))
    except urllib.error.HTTPError as error:
        if error.code != 404:
            raise
        print(f"Skipping missing asset: {url}", file=sys.stderr)
        temporary.unlink(missing_ok=True)
        return False
    temporary.replace(destination)
    return True


def extract_archive(archive: Path, destination: Path) -> list[Path]:
    extracted: list[Path] = []
    with zipfile.ZipFile(archive) as files:
        for member in files.infolist():
            member_path = Path(member.filename)
            if (
                member.is_dir()
                or "__MACOSX" in member_path.parts
                or member_path.name.startswith("._")
                or member_path.suffix.lower()
                not in {
                    ".aif",
                    ".aiff",
                    ".wav",
                }
            ):
                continue
            name = member_path.name
            target = destination / name
            target.parent.mkdir(parents=True, exist_ok=True)
            with files.open(member) as source, target.open("wb") as output:
                shutil.copyfileobj(source, output)
            extracted.append(target)
    return extracted


def convert_to_wav(source: Path, destination: Path) -> None:
    if destination.exists():
        return
    destination.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            "ffmpeg",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            str(source),
            "-ar",
            "44100",
            "-ac",
            "1",
            "-c:a",
            "pcm_s16le",
            str(destination),
        ],
        check=True,
    )


def note_pitch(name: str) -> int:
    semitones = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}
    match = re.fullmatch(r"([A-G])([b#]?)(-?\d+)", name)
    if match is None:
        raise ValueError(f"Invalid note name: {name}")
    letter, accidental, octave = match.groups()
    return (int(octave) + 1) * 12 + semitones[letter] + {"b": -1, "": 0, "#": 1}[accidental]


def pitch_name(pitch: int) -> str:
    names = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"]
    return f"{names[pitch % 12]}{pitch // 12 - 1}"


def split_chromatic_scale(source: Path) -> bool:
    parts = source.stem.split(".")
    range_part = next((part for part in parts if NOTE_RANGE.fullmatch(part)), None)
    if range_part is None:
        return False
    match = NOTE_RANGE.fullmatch(range_part)
    assert match is not None
    first_pitch = note_pitch(match.group(1))
    last_pitch = note_pitch(match.group(2))
    if last_pitch < first_pitch:
        print(f"Cannot split descending range {source.name}", file=sys.stderr)
        return False
    expected = last_pitch - first_pitch + 1
    intervals = []
    duration = 0.0
    detected_counts = set()
    for threshold in [-45, -40, -35, -30, -25, -50, -55, -60]:
        for silence_duration in [0.5, 0.7, 0.9, 0.3]:
            analysis = subprocess.run(
                [
                    "ffmpeg",
                    "-hide_banner",
                    "-i",
                    str(source),
                    "-af",
                    f"silencedetect=noise={threshold}dB:d={silence_duration}",
                    "-f",
                    "null",
                    "-",
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            if analysis.returncode != 0:
                print(f"Could not analyse chromatic scale {source}", file=sys.stderr)
                return False
            duration_match = re.search(
                r"Duration: (\d+):(\d+):(\d+(?:\.\d+)?)", analysis.stderr
            )
            if duration_match is None:
                print(f"Could not determine duration of {source}", file=sys.stderr)
                return False
            duration = (
                int(duration_match.group(1)) * 3_600
                + int(duration_match.group(2)) * 60
                + float(duration_match.group(3))
            )
            silence_starts = [
                float(value)
                for value in re.findall(r"silence_start: ([0-9.]+)", analysis.stderr)
            ]
            silence_ends = [
                float(value)
                for value in re.findall(r"silence_end: ([0-9.]+)", analysis.stderr)
            ]
            candidate = []
            cursor = 0.0
            for silence_start, silence_end in zip(
                silence_starts, silence_ends, strict=True
            ):
                if silence_start > cursor + 0.05:
                    candidate.append((cursor, silence_start))
                cursor = silence_end
            if cursor < duration - 0.05:
                candidate.append((cursor, duration))
            detected_counts.add(len(candidate))
            if len(candidate) == expected:
                intervals = candidate
                break
        if intervals:
            break
    if not intervals:
        print(
            f"Skipping {source.name}: detected counts {sorted(detected_counts)}, expected {expected}",
            file=sys.stderr,
        )
        return False

    temporary_outputs = []
    for offset, (start, end) in enumerate(intervals):
        output_parts = [pitch_name(first_pitch + offset) if part == range_part else part for part in parts]
        destination = source.with_name(".".join(output_parts) + ".wav")
        temporary = destination.with_suffix(".wav.part")
        subprocess.run(
            [
                "ffmpeg",
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-ss",
                f"{max(0.0, start - 0.02):.6f}",
                "-to",
                f"{min(duration, end + 0.05):.6f}",
                "-i",
                str(source),
                "-c:a",
                "pcm_s16le",
                "-f",
                "wav",
                str(temporary),
            ],
            check=True,
        )
        temporary_outputs.append((temporary, destination))
    for temporary, destination in temporary_outputs:
        temporary.replace(destination)
    source.unlink()
    print(f"Split {source.name} into {expected} notes")
    return True


def split_downloaded_scales(output_root: Path) -> int:
    split_count = 0
    for source in sorted(output_root.rglob("*.wav")):
        split_count += int(split_chromatic_scale(source))
    return split_count


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--instrument",
        action="append",
        default=[],
        help="download page names containing this text; may be repeated",
    )
    parser.add_argument("--list", action="store_true", help="list instrument page names")
    parser.add_argument(
        "--process-existing",
        action="store_true",
        help="split previously downloaded chromatic-scale WAVs without downloading",
    )
    args = parser.parse_args()

    if shutil.which("ffmpeg") is None and not args.list:
        parser.error("ffmpeg is required to convert the Iowa AIFF recordings to WAV")

    root = Path(__file__).resolve().parent
    output_root = root / "iowa"
    if args.process_existing:
        count = split_downloaded_scales(output_root)
        print(f"Split {count} chromatic-scale recording(s)")
        return 0

    pages = instrument_pages()
    if args.list:
        print("\n".join(sorted(pages)))
        return 0

    requested = [value.lower() for value in args.instrument]
    selected = {
        name: url
        for name, url in pages.items()
        if not requested or any(value in name for value in requested)
    }
    if not selected:
        parser.error("no instrument page matched --instrument")

    cache = root / "_downloads"
    manifest: list[dict[str, object]] = []
    for name, page_url in sorted(selected.items()):
        links = page_links(page_url)
        archives = [url for url, _ in links if url.lower().endswith(".zip")]
        preferred = [
            url
            for url in archives
            if "mono" in url.lower() and ("1644" in url.lower() or "16.44" in url.lower())
        ]
        asset_urls = preferred or archives
        if not asset_urls:
            asset_urls = [
                url
                for url, _ in links
                if Path(urllib.parse.urlparse(url).path).suffix.lower()
                in {".aif", ".aiff", ".wav"}
            ]
        if not asset_urls:
            print(f"Skipping {name}: no downloadable audio found", file=sys.stderr)
            continue

        instrument_output = output_root / name
        source_files: list[Path] = []
        for asset_url in dict.fromkeys(asset_urls):
            filename = Path(urllib.parse.unquote(urllib.parse.urlparse(asset_url).path)).name
            cached = cache / name / filename
            if not download(asset_url, cached):
                continue
            if cached.suffix.lower() == ".zip":
                source_files.extend(extract_archive(cached, cache / name / "extracted"))
            else:
                source_files.append(cached)

        for source in source_files:
            convert_to_wav(source, instrument_output / f"{source.stem}.wav")
        split_downloaded_scales(instrument_output)
        manifest.append(
            {
                "instrument": name,
                "page": page_url,
                "assets": list(dict.fromkeys(asset_urls)),
                "wav_files": sorted(path.name for path in instrument_output.glob("*.wav")),
            }
        )

    output_root.mkdir(parents=True, exist_ok=True)
    (output_root / "sources.json").write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
    )
    print(f"Wrote {output_root / 'sources.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
