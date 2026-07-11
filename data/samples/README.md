# Sample data

This directory contains scripts and attribution for optional sample libraries. The
audio itself is deliberately ignored by Git because it is large and can be restored
from its original source.

Download and convert the University of Iowa Musical Instrument Samples with:

```sh
./data/samples/download_iowa_samples.py
```

The script requires Python 3 and `ffmpeg`. By default it downloads every instrument
page in the collection. Pass `--instrument violin` (the option may be repeated) to
download only matching instruments, or `--list` to see the available page names.

Downloaded archives are cached under `_downloads/`. WAV files and a machine-readable
`sources.json` manifest are written under `iowa/`. Both directories are ignored.

See [ATTRIBUTION.md](ATTRIBUTION.md) for the source and credit that must accompany a
distributed sample bundle.
