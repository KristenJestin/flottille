#!/usr/bin/env python3
"""Makes Flottille's two notification sounds, `needs-you.wav` and `tell.wav`, from nothing but
sine waves: they are ours, under the licence of the repository (AGPL-3.0-only), with nothing
sampled or taken from anywhere. Short, mono, 16 bits, 22.05 kHz, and quiet (a peak of about 0.35
of full scale, so that a volume setting only has to lower them).

    python3 assets/sounds/make.py

writes the two files beside this script; the same script, run again, writes the same bytes.
"""

import math
import struct
import sys
import wave
from pathlib import Path

RATE = 22_050
PEAK = 0.35


def note(frequency, seconds, start, total):
    """A soft bell: a sine and its octave, an attack of 8 ms and an exponential decay."""
    samples = [0.0] * total
    first = int(start * RATE)
    count = int(seconds * RATE)
    for i in range(count):
        t = i / RATE
        attack = min(1.0, t / 0.008)
        decay = math.exp(-t * 11.0)
        wave_ = math.sin(2 * math.pi * frequency * t) + 0.25 * math.sin(
            2 * math.pi * 2 * frequency * t
        )
        if first + i < total:
            samples[first + i] += attack * decay * wave_
    return samples


def mix(parts, total):
    mixed = [sum(part[i] for part in parts) for i in range(total)]
    peak = max(abs(sample) for sample in mixed) or 1.0
    return [sample / peak * PEAK for sample in mixed]


def write(path, samples):
    with wave.open(str(path), "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(RATE)
        out.writeframes(b"".join(struct.pack("<h", round(s * 32767)) for s in samples))


def main():
    here = Path(sys.argv[0]).resolve().parent
    # What needs the user (a request, a question): two notes going up, E5 then A5.
    total = int(0.42 * RATE)
    write(
        here / "needs-you.wav",
        mix([note(659.25, 0.3, 0.0, total), note(880.0, 0.3, 0.12, total)], total),
    )
    # What only tells (a report, a failure, a pull request, a ticket): one note, G5.
    total = int(0.22 * RATE)
    write(here / "tell.wav", mix([note(783.99, 0.22, 0.0, total)], total))


main()
