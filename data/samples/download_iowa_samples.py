#!/usr/bin/env python3
"""Download Iowa MIS instruments and convert their audio to sampler-ready WAV."""

import argparse
import json
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


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--instrument",
        action="append",
        default=[],
        help="download page names containing this text; may be repeated",
    )
    parser.add_argument("--list", action="store_true", help="list instrument page names")
    args = parser.parse_args()

    if shutil.which("ffmpeg") is None and not args.list:
        parser.error("ffmpeg is required to convert the Iowa AIFF recordings to WAV")

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

    root = Path(__file__).resolve().parent
    cache = root / "_downloads"
    output_root = root / "iowa"
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
        # TODO: Split Iowa chromatic-scale recordings (for example C2B2) into individual
        # note WAVs using onset/silence detection. The DAW intentionally refuses to map a
        # whole scale recording as though it were one pitched sample.
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
