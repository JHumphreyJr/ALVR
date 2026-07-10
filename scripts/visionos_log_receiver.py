#!/usr/bin/env python3
"""Receive visionOS/Xcode logs over TCP and collapse noisy repeats."""

from __future__ import annotations

import argparse
import re
import sys
import time
from collections import defaultdict
from datetime import datetime
from pathlib import Path
from socketserver import StreamRequestHandler, ThreadingTCPServer

# Substrings (or regex) to collapse instead of printing every line.
COLLAPSE_PATTERNS: list[re.Pattern[str]] = [
    re.compile(p, re.IGNORECASE)
    for p in [
        r"IPD is bad, no frame",
        r"Missing video format, no frame",
        r"Origin is bad, no frame",
        r"mDNS registration updated",
        r"add\(ALVR Apple Vision Pro",
    ]
]


def should_collapse(line: str) -> re.Pattern[str] | None:
    for pattern in COLLAPSE_PATTERNS:
        if pattern.search(line):
            return pattern
    return None


class CollapsingWriter:
    def __init__(self, out_path: Path | None, flush_interval: float) -> None:
        self.out_path = out_path
        self.flush_interval = flush_interval
        self.counts: dict[str, int] = defaultdict(int)
        self.last_flush = time.monotonic()
        self._out = sys.stdout

    def _emit(self, message: str) -> None:
        stamped = f"[{datetime.now().strftime('%H:%M:%S')}] {message}"
        print(stamped, flush=True)
        if self.out_path is not None:
            with self.out_path.open("a", encoding="utf-8") as f:
                f.write(stamped + "\n")

    def write_line(self, line: str) -> None:
        line = line.rstrip("\r\n")
        if not line:
            return

        pattern = should_collapse(line)
        if pattern is not None:
            self.counts[pattern.pattern] += 1
            now = time.monotonic()
            if now - self.last_flush >= self.flush_interval:
                self.flush_collapsed()
                self.last_flush = now
            return

        self.flush_collapsed()
        self._emit(line)

    def flush_collapsed(self) -> None:
        if not self.counts:
            return
        for key, count in sorted(self.counts.items()):
            label = key.replace(r"\.", ".").replace(r", no frame", "")
            self._emit(f"… collapsed {count}× {label}")
        self.counts.clear()
        self.last_flush = time.monotonic()


class LogHandler(StreamRequestHandler):
    writer: CollapsingWriter | None = None

    def handle(self) -> None:
        assert LogHandler.writer is not None
        peer = f"{self.client_address[0]}:{self.client_address[1]}"
        LogHandler.writer._emit(f"client connected ({peer})")
        try:
            for raw in self.rfile:
                try:
                    line = raw.decode("utf-8", errors="replace")
                except Exception:
                    continue
                LogHandler.writer.write_line(line)
        finally:
            LogHandler.writer.flush_collapsed()
            LogHandler.writer._emit(f"client disconnected ({peer})")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="0.0.0.0", help="bind address")
    parser.add_argument("--port", type=int, default=9920, help="listen port")
    parser.add_argument(
        "--out",
        type=Path,
        default=Path("/tmp/visionos_xcode.log"),
        help="append filtered log to this file",
    )
    parser.add_argument(
        "--flush-interval",
        type=float,
        default=3.0,
        help="seconds between collapsed summaries",
    )
    args = parser.parse_args()

    LogHandler.writer = CollapsingWriter(args.out, args.flush_interval)
    LogHandler.writer._emit(
        f"listening on {args.host}:{args.port} → {args.out}"
    )

    with ThreadingTCPServer((args.host, args.port), LogHandler) as server:
        server.serve_forever()


if __name__ == "__main__":
    main()
