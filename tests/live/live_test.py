#!/usr/bin/env python3
"""Live tests: the real pane in a real Claude Code, driven through tmux.

What `claude plugin test` cannot reach: the focus ring, Escape, the dock,
widths, and the files the CLI really moves. Claude Code runs with the user's
own login; the mod's `cliPath` points at a wrapper that sets
CLAUDE_CONFIG_DIR to a generated config, so the pane only ever sees and
changes fake sessions.

    python3 tests/live/live_test.py            # every scenario
    python3 tests/live/live_test.py -k esc     # names containing "esc"

Each scenario starts its own Claude Code (about 10 s). Claude Code itself
records a short session per start under the real config, in the project of
CWD below; clear them with the pane's `e` there when they pile up.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import traceback
import uuid
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

REPO = Path(__file__).resolve().parents[2]
# Fixed, so the trust prompt is answered once and the real config gains one project.
CWD = Path("/tmp/sessile-live-cwd")
OTHER = "/work/other"
# The filter's placeholder: drawn only while the field is empty.
EMPTY_FILTER = r"›\s*:\s*filter · Enter"
# The first row of the default fixture is pinned.
FIRST_ROW_PIN = "p [ unpin ]"
LEGEND = r"★ pinned\s+◆ this\s+● live\s+✎ named"
NOW = time.time()


def slug(path: str) -> str:
    return "".join(c if c.isascii() and c.isalnum() else "-" for c in path)


# ---------------------------------------------------------------- fixtures


@dataclass
class Session:
    title: str
    prompts: int = 3
    pad: int = 30_000
    text: str = ""
    project: str = str(CWD)
    is_custom: bool = False
    id: str = field(default_factory=lambda: str(uuid.uuid4()))


def compact(value: object) -> str:
    """JSON as Claude Code writes it; the CLI matches entry types byte for byte."""
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False)


def session_lines(s: Session) -> list[str]:
    lines = []
    for i in range(s.prompts):
        prompt = f"{s.title} prompt {i}" if i else f"{s.title}: first prompt"
        lines.append(compact({
            "type": "user", "message": {"role": "user", "content": prompt},
            "cwd": s.project, "gitBranch": "main", "sessionId": s.id,
        }))
        reply = f"reply {i} {s.text} " + "x" * (s.pad // max(s.prompts, 1))
        lines.append(compact({
            "type": "assistant",
            "message": {"role": "assistant", "model": "claude-test", "content": [{"type": "text", "text": reply}]},
            "cwd": s.project, "sessionId": s.id,
        }))
    if s.prompts == 0:
        lines.append(compact({"type": "summary", "summary": "nothing", "cwd": s.project}))
    kind = "custom-title" if s.is_custom else "ai-title"
    key = "customTitle" if s.is_custom else "aiTitle"
    lines.append(compact({"type": kind, key: s.title, "sessionId": s.id}))
    return lines


@dataclass
class Fixture:
    root: Path
    sessions: list[Session]

    @property
    def config(self) -> Path:
        return self.root / "config"

    @property
    def export_dir(self) -> Path:
        return self.root / "export"

    def by_title(self, title: str) -> Session:
        return next(s for s in self.sessions if s.title == title)

    def transcript(self, s: Session, archived: bool = False) -> Path:
        # The archive keeps the footprint's paths relative to the config.
        base = self.config / "session-archive" / slug(s.project) if archived else self.config
        return base / "projects" / slug(s.project) / f"{s.id}.jsonl"

    def pins(self) -> list[str]:
        path = self.config / "sessile" / "pins.json"
        return json.loads(path.read_text())["pinned"] if path.exists() else []


def default_sessions(bulk: int = 0) -> list[Session]:
    sessions = [
        Session("alpha deploy notes", prompts=5, is_custom=True),
        Session("beta refactor plan"),
        Session("gamma zebracorn hunt", text="the zebracorn lives here"),
        Session("delta live one"),
        Session("epsilon throwaway"),
        Session("junk tiny", prompts=1, pad=100),
        Session("empty one", prompts=0, pad=0),
        Session("empty two", prompts=0, pad=0),
        Session("empty but pinned", prompts=0, pad=0),
    ]
    sessions += [Session(f"session {i:04d}") for i in range(bulk or 30)]
    sessions += [Session(f"other project {i}", project=OTHER) for i in range(5)]
    return sessions


def build_fixture(root: Path, sessions: list[Session]) -> Fixture:
    fx = Fixture(root, sessions)
    for i, s in enumerate(sessions):
        path = fx.transcript(s)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("\n".join(session_lines(s)) + "\n")
        # Newest first in list order.
        os.utime(path, (NOW - i * 600, NOW - i * 600))
    live = fx.by_title("delta live one")
    (fx.config / "sessions").mkdir(parents=True, exist_ok=True)
    # This process is alive for as long as the test runs.
    (fx.config / "sessions" / f"{os.getpid()}.json").write_text(json.dumps({"pid": os.getpid(), "sessionId": live.id}))
    (fx.config / "sessile").mkdir(parents=True, exist_ok=True)
    pinned = fx.by_title("empty but pinned")
    (fx.config / "sessile" / "pins.json").write_text(json.dumps({"pinned": [pinned.id]}))
    fx.export_dir.mkdir(parents=True, exist_ok=True)
    return fx


# ---------------------------------------------------------------- tmux


class Failure(AssertionError):
    pass


class Screen:
    """One Claude Code in a detached tmux session."""

    def __init__(self, name: str, width: int, height: int) -> None:
        self.name = name
        self.width = width
        self.height = height

    def tmux(self, *args: str) -> str:
        return subprocess.run(["tmux", *args], check=True, capture_output=True, text=True).stdout

    def start(self, command: list[str]) -> None:
        CWD.mkdir(parents=True, exist_ok=True)
        cmd = " ".join(shlex_quote(c) for c in command)
        self.tmux("new-session", "-d", "-s", self.name, "-x", str(self.width), "-y", str(self.height), "-c", str(CWD), cmd)

    def stop(self) -> None:
        subprocess.run(["tmux", "kill-session", "-t", self.name], capture_output=True)

    def keys(self, *keys: str, pause: float = 0.4) -> None:
        for key in keys:
            self.tmux("send-keys", "-t", self.name, key)
            time.sleep(pause)

    def repeat(self, key: str, count: int, pause: float = 0.4) -> None:
        self.tmux("send-keys", "-t", self.name, "-N", str(count), key)
        time.sleep(pause)

    def type(self, text: str, pause: float = 0.6) -> None:
        self.tmux("send-keys", "-t", self.name, "-l", text)
        time.sleep(pause)

    def click_at(self, x: int, y: int, pause: float = 1.2) -> None:
        """A left click as an SGR mouse report, 1-based cells."""
        self.tmux("send-keys", "-t", self.name, "-l", f"\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m")
        time.sleep(pause)

    def click(self, text: str) -> None:
        for y, line in enumerate(self.lines()):
            x = line.find(text)
            if x >= 0:
                return self.click_at(x + 1, y + 1)
        raise Failure(f"nothing to click: no {text!r} on screen:\n{self.text()}")

    def click_prompt(self) -> None:
        y = max(i for i, line in enumerate(self.lines()) if line.startswith("❯"))
        self.click_at(3, y + 1)

    def resize(self, width: int, height: int | None = None) -> None:
        self.width = width
        self.height = height or self.height
        self.tmux("resize-window", "-t", self.name, "-x", str(width), "-y", str(self.height))
        time.sleep(1.5)

    def lines(self) -> list[str]:
        return self.tmux("capture-pane", "-t", self.name, "-p").split("\n")

    def text(self) -> str:
        return "\n".join(self.lines())

    def pane(self) -> list[str]:
        """The lines right of the dock border; every line when nothing is docked."""
        lines = self.lines()
        counts = Counter(line.index("│") for line in lines if "│" in line)
        if not counts:
            return lines
        border, hits = counts.most_common(1)[0]
        if hits < len(lines) // 2:
            return lines
        return [line[border + 1:] if len(line) > border else "" for line in lines]

    def pane_text(self) -> str:
        return "\n".join(self.pane())

    def prompt(self) -> str:
        """What the composer holds."""
        for line in reversed(self.lines()):
            m = re.match(r"^❯ ?(.*?)(\s*│.*)?$", line)
            if m:
                return m.group(1).strip()
        return ""

    def wait(self, pattern: str, timeout: float = 10, where: str = "pane") -> re.Match[str]:
        deadline = time.time() + timeout
        while True:
            text = self.pane_text() if where == "pane" else self.text()
            m = re.search(pattern, text, re.M)
            if m:
                return m
            if time.time() > deadline:
                raise Failure(f"no /{pattern}/ within {timeout} s; {where} was:\n{text}")
            time.sleep(0.2)

    def wait_gone(self, pattern: str, timeout: float = 10) -> None:
        deadline = time.time() + timeout
        while re.search(pattern, self.pane_text(), re.M):
            if time.time() > deadline:
                raise Failure(f"/{pattern}/ still there after {timeout} s:\n{self.pane_text()}")
            time.sleep(0.2)

    def has(self, pattern: str) -> bool:
        return re.search(pattern, self.pane_text(), re.M) is not None

    def expect(self, pattern: str, why: str) -> None:
        if not self.has(pattern):
            raise Failure(f"{why}: no /{pattern}/ in the pane:\n{self.pane_text()}")

    def expect_not(self, pattern: str, why: str) -> None:
        if self.has(pattern):
            raise Failure(f"{why}: /{pattern}/ is in the pane:\n{self.pane_text()}")

    def expect_prompt_empty(self, why: str) -> None:
        if self.prompt() != "":
            raise Failure(f"{why}: the composer holds {self.prompt()!r}:\n{self.text()}")


def shlex_quote(s: str) -> str:
    import shlex

    return shlex.quote(s)


# ---------------------------------------------------------------- session


@dataclass
class Run:
    screen: Screen
    fx: Fixture

    def open(self) -> None:
        self.screen.type("/sessile", pause=0.8)
        self.screen.keys("Enter")
        self.screen.wait(r"(Sessions|Archive) · \d+", timeout=15)
        self.screen.wait(r"Type to filter|live session|rename")
        # The first frame shows "· 0" before the list is read; a click then
        # lands in the middle of that reload.
        self.screen.wait(r"\d+-\d+ of \d+")
        time.sleep(0.5)

    def clear_filter(self) -> None:
        """Ring in an empty filter."""
        s = self.screen
        if not s.has(r"Type to filter"):
            s.keys("f")
            s.wait(r"Type to filter")
        if not s.has(EMPTY_FILTER):
            s.repeat("BSpace", 60)
            s.wait(EMPTY_FILTER)

    def to_row(self, title: str) -> None:
        """Ring onto the row titled `title`, through the filter."""
        self.clear_filter()
        self.screen.type(title)
        self.screen.wait(r"of 1\b")
        self.screen.keys("Down")
        self.screen.wait(r"copy id")

    def position(self) -> tuple[int, int, int]:
        """The header's `first-last of total`."""
        m = re.search(r"(\d+)-(\d+) of (\d+)", self.screen.pane_text())
        if not m:
            raise Failure(f"no position in the header:\n{self.screen.pane_text()}")
        return int(m.group(1)), int(m.group(2)), int(m.group(3))


def start(args: argparse.Namespace, name: str, fx: Fixture, width: int = 150, height: int = 45, layout: str = "pane") -> Run:
    wrapper = fx.root / "sessile-wrapper.sh"
    wrapper.write_text(f'#!/bin/sh\nCLAUDE_CONFIG_DIR="{fx.config}" exec "{args.cli}" "$@"\n')
    wrapper.chmod(0o755)
    settings = fx.root / "settings.json"
    options = {"cliPath": str(wrapper), "exportDir": str(fx.export_dir), "layout": layout}
    settings.write_text(json.dumps({"pluginConfigs": {"sessile": {"options": options}}}))
    screen = Screen(f"sessile-live-{os.getpid()}-{name}", width, height)
    screen.start(["claude", "--plugin-dir", str(args.mod), "--settings", str(settings)])
    deadline = time.time() + 30
    while time.time() < deadline:
        text = screen.text()
        if "trust this folder" in text:
            screen.keys("Down", "Enter")
        elif re.search(r"^❯", text, re.M) and re.search(r"shift\+tab|for shortcuts", text):
            break
        time.sleep(0.5)
    else:
        raise Failure(f"Claude Code did not start:\n{screen.text()}")
    time.sleep(1)
    return Run(screen, fx)


# ---------------------------------------------------------------- scenarios


def s_open_and_keys(run: Run) -> None:
    s = run.screen
    run.open()
    s.expect(r"Type to filter", "the hint teaches the filter at open")
    s.type("v")
    s.expect(r"Sessions ·", "v typed into the filter does not switch to the archive")
    s.expect(r"›\s*:?\s*v\b", "v lands in the filter")
    s.keys("Escape")
    s.wait(EMPTY_FILTER)
    s.expect_prompt_empty("Esc in a filled filter keeps the keys in the pane")
    s.keys("Down")
    s.wait(r"copy id")
    s.keys("v")
    s.wait(r"Archive ·")
    s.keys("v")
    s.wait(r"Sessions ·")


def s_toggle_on_empty_view(run: Run) -> None:
    s = run.screen
    run.open()
    s.keys("Down")
    s.wait(r"copy id")
    s.keys("v")
    s.wait(r"Archive · 0")
    # The ring stays on `v`, so the same key comes back.
    s.keys("v")
    s.wait(r"Sessions · \d+")


def s_moving_speed(run: Run) -> None:
    s = run.screen
    run.open()
    s.wait(r"of 3009")
    s.keys("Down")
    s.wait(r"copy id")
    # A held arrow: 60 presses at the usual key repeat of 30 a second.
    for _ in range(60):
        s.tmux("send-keys", "-t", s.name, "Down")
        time.sleep(1 / 30)
    released = time.time()
    while run.position()[1] < 61:
        if time.time() - released > 5:
            raise Failure(f"60 arrows not done 5 s after the release: {run.position()}")
        time.sleep(0.05)
    late = time.time() - released
    print(f"  the ring stopped {late:.2f} s after the release")
    if late > 0.5:
        raise Failure(f"the ring ran on {late:.1f} s after the release")
    time.sleep(1)
    if run.position()[1] != 61:
        raise Failure(f"the ring ran on past row 61: {run.position()}")
    first = run.position()[0]
    s.keys("n")
    time.sleep(1)
    if run.position()[0] <= first:
        raise Failure(f"n did not page down: {run.position()}")
    s.keys("b")
    time.sleep(1)
    if run.position()[0] != first:
        raise Failure(f"b did not page back: {run.position()} vs start {first}")


def s_filter_search_escape(run: Run) -> None:
    s = run.screen
    run.open()
    s.type("alpha")
    s.wait(r"of 1\b")
    s.expect(r"alpha deploy notes", "the fuzzy filter finds the title")
    s.keys("Escape")
    s.wait(EMPTY_FILTER)
    s.expect_prompt_empty("Esc in the filter keeps the keys")
    # Full text: the word is only in a reply.
    s.type("zebracorn")
    s.keys("Enter")
    s.wait(r"full-text “zebracorn” · 1 hits")
    s.keys("Down")
    s.wait(r"copy id")
    s.keys("v")
    s.wait(r"Archive ·")
    s.expect(r"full-text “zebracorn” · 0 hits", "the search runs again in the archive")
    s.keys("v")
    s.wait(r"full-text “zebracorn” · 1 hits")
    s.keys("c")
    s.wait_gone(r"full-text “")
    # Esc on a row with a filter: the filter clears, the ring stays on the row.
    run.clear_filter()
    s.type("session 00")
    s.wait(r"of \d+")
    s.keys("Down", "Down")
    s.wait(r"copy id")
    s.keys("Escape")
    s.wait(r"of 39\b")
    s.expect(r"copy id", "the ring stays on the row")
    s.expect_prompt_empty("Esc on a row keeps the keys")
    s.keys("Escape")
    s.wait_gone(r"Sessions ·")


def s_detail(run: Run) -> None:
    s = run.screen
    run.open()
    s.keys("Down", "Enter")
    s.wait(r"FIRST")
    s.expect(r"esc \[ back \]", "detail offers esc back")
    for button in ("r [ resume ]", "t [ rename ]", FIRST_ROW_PIN, "i [ copy id ]", "q [ close ]"):
        s.expect(re.escape(button), "every detail button is drawn whole")
    s.keys("Escape")
    s.wait_gone(r"FIRST")
    s.wait(r"copy id")
    s.keys("Down")
    s.expect_prompt_empty("after Esc in the detail the arrows still move in the pane")
    s.expect(r"copy id", "the ring is on a row")
    s.keys("Enter")
    s.wait(r"FIRST")
    s.keys("Enter")
    s.wait_gone(r"FIRST")
    s.keys("Enter")
    s.wait(r"FIRST")
    s.keys("q")
    s.wait_gone(r"FIRST|Sessions ·")
    run.open()
    run.to_row("delta live one")
    s.keys("Enter")
    s.wait(r"Live session: resume, rename, archive and delete are")


def s_changes(run: Run) -> None:
    s, fx = run.screen, run.fx
    run.open()
    beta = fx.by_title("beta refactor plan")
    run.to_row(beta.title)
    s.keys("t")
    s.wait(r"Rename session")
    s.repeat("BSpace", 40)
    s.type("renamed live")
    s.keys("Enter")
    s.wait(r"Renamed to")
    last = json.loads(fx.transcript(beta).read_text().strip().split("\n")[-1])
    if last != {"type": "custom-title", "customTitle": "renamed live", "sessionId": beta.id}:
        raise Failure(f"rename wrote {last}")

    eps = fx.by_title("epsilon throwaway")
    run.to_row(eps.title)
    s.keys("a")
    s.wait(r"Archived “epsilon")
    s.expect(r"u\s*\[?\s*undo", "archive offers undo")
    if not fx.transcript(eps, archived=True).exists() or fx.transcript(eps).exists():
        raise Failure("archive did not move the transcript")
    s.keys("u")
    s.wait(r"Restored “epsilon")
    if not fx.transcript(eps).exists():
        raise Failure("undo did not move the transcript back")

    s.wait(r"copy id")
    s.keys("p")
    s.wait(r"unpin")
    if eps.id not in fx.pins():
        raise Failure(f"pin not stored: {fx.pins()}")
    s.keys("d")
    s.wait(r"Delete permanently\?")
    s.expect(r"pinned; deleting drops the pin", "delete warns about the pin")
    s.keys("Escape")
    s.wait_gone(r"Delete permanently")
    s.expect_prompt_empty("Esc in the dialog keeps the keys")
    s.wait(r"copy id")
    s.keys("d")
    s.wait(r"Delete permanently\?")
    s.keys("y")
    s.wait(r"Deleted “epsilon")
    if fx.transcript(eps).exists() or eps.id in fx.pins():
        raise Failure("delete left the transcript or the pin")

    run.clear_filter()
    s.keys("Down")
    s.wait(r"copy id")
    s.keys("e")
    s.wait(r"Delete 2 empty sessions permanently\?")
    s.expect_not(r"empty but pinned", "a pinned empty session is not offered")
    s.keys("y")
    s.wait(r"Deleted 2 empty sessions")
    for title in ("empty one", "empty two"):
        if fx.transcript(fx.by_title(title)).exists():
            raise Failure(f"{title} not deleted")
    if not fx.transcript(fx.by_title("empty but pinned")).exists():
        raise Failure("the pinned empty session was deleted")


def s_export_and_copy(run: Run) -> None:
    s, fx = run.screen, run.fx
    run.open()
    run.to_row("alpha deploy notes")
    s.keys("m")
    s.wait(r"Exported \S+\.md to")
    files = list(fx.export_dir.glob("*.md"))
    if len(files) != 1 or "## You" not in files[0].read_text():
        raise Failure(f"export wrote {files}")
    s.keys("i")
    s.wait(r"Copied session id|Copy failed")
    if s.has(r"Copy failed"):
        print("  note: the clipboard is not reachable from tmux; copy needs a person")


def s_widths(run: Run) -> None:
    s = run.screen
    run.open()
    s.keys("Down")
    s.wait(r"copy id")

    # Whole, as `k [ label ]`: a squeezed line drops the space or eats the label.
    buttons = ["v [ show archive ]", "w [ all projects ]", "x [ junk ]", "e [ delete empty ]", "f [ filter ]",
               "g [ reload ]", "q [ close ]", "r [ resume ]", "t [ rename ]", "a [ archive ]", "d [ delete ]",
               FIRST_ROW_PIN, "m [ export md ]", "i [ copy id ]"]
    # An inline pane is low: the compact list draws the same keys as `k: label`.
    compact = [re.sub(r"^(\w) \[ (.*) \]$", r"\1: \2", button) for button in buttons]
    for columns in (150, 400, 100, 220):
        s.resize(columns)
        s.wait(r"q \[ close \]|q: close")
        width = max(len(line.rstrip()) for line in s.pane())
        is_compact = s.has(r"q: close")
        print(f"  terminal {columns}: pane ~{width} columns{', compact' if is_compact else ''}")
        for button in compact if is_compact else buttons:
            s.expect(re.escape(button), f"at {columns} columns every button is drawn whole")
        if is_compact:
            # Its top line is the header, or the notice that the prompt has the keys.
            s.expect(r"beta refactor plan", "the compact list still shows rows")
        else:
            # A pane re-seated by the resize may have lost the keys: its legend line says so.
            s.expect(f"{LEGEND}|The keys are with the prompt", f"at {columns} columns the legend line stays drawn")


def s_mouse(run: Run) -> None:
    s = run.screen
    run.open()
    s.click("beta refactor plan")
    s.expect_not(r"FIRST", "a first click selects, it does not open")
    s.expect(r"copy id", "the clicked row shows its actions")
    s.click("gamma zebracorn hunt")
    s.expect_not(r"FIRST", "a click on another row selects it")
    s.click("gamma zebracorn hunt")
    s.wait(r"FIRST")
    s.expect(r"gamma zebracorn hunt", "the second click opens the clicked row")
    s.keys("Escape")
    s.wait_gone(r"FIRST")
    # Escape's way back takes the keys back too; a click before it ends loses them to it.
    s.wait(r"copy id")
    time.sleep(1)
    s.click_prompt()
    s.type("z")
    if s.prompt() != "z":
        raise Failure(f"the click on the prompt did not give it the keys:\n{s.text()}")
    s.keys("BSpace")
    s.click("epsilon throwaway")
    s.expect_not(r"FIRST", "the click that hands the pane its keys only selects")
    s.wait(r"copy id")
    s.keys("Down")
    s.expect_prompt_empty("after that click the keys are in the pane")
    s.click("alpha deploy notes")
    s.click("alpha deploy notes")
    s.wait(r"FIRST")


def real_session(prompt: str) -> str:
    """A session in the real config under CWD: `/resume` reads the config
    Claude Code runs on, not the fixture the CLI sees. One short Haiku call."""
    out = subprocess.run(
        ["claude", "-p", prompt, "--model", "claude-haiku-4-5-20251001", "--max-turns", "1", "--output-format", "json"],
        cwd=CWD, capture_output=True, text=True, timeout=120,
    )
    if out.returncode != 0:
        raise Failure(f"could not make a real session: {out.stderr or out.stdout}")
    return json.loads(out.stdout)["session_id"]


def s_resume(run: Run) -> None:
    s, fx = run.screen, run.fx
    # The same id in both configs: the pane lists the fixture, /resume loads the real one.
    target = Session("resume target", id=real_session("Reply with the single word: kilo"))
    fx.sessions.append(target)
    path = fx.transcript(target)
    path.write_text("\n".join(session_lines(target)) + "\n")
    run.open()
    run.to_row(target.title)
    s.keys("r")
    s.wait(r"Resumed “resume target”", timeout=30)
    s.wait(r"Reply with the single word: kilo", where="all")
    # The resumed session is now this one, so its row is read-only. The ring
    # moves to the row only after the notice is drawn.
    s.wait(r"live session: read-only", timeout=5)
    s.keys("Down")
    s.expect_prompt_empty("after a resume the keys stay in the pane")

    s.keys("w")
    s.wait(r"all projects ·")
    # The filter also matches ids, which are random: a digit in it would make
    # the count depend on them.
    run.clear_filter()
    s.type("other project")
    s.wait(r"1-5 of 5")
    s.keys("Down")
    s.wait(r"copy id")
    s.keys("r")
    s.wait(r"a new terminal: cd '")
    s.expect_not(r"Resumed “other", "another project's session is not resumed in place")


def s_guards(run: Run) -> None:
    s, fx = run.screen, run.fx
    run.open()
    run.to_row("delta live one")
    s.expect(r"live session: read-only", "a live row is read-only")
    s.expect_not(r"rename \]|resume \]", "no rename or resume on a live row")
    live = fx.by_title("delta live one")
    wrapper = fx.root / "sessile-wrapper.sh"
    out = subprocess.run([str(wrapper), "--project", str(CWD), "--json", "archive", live.id], capture_output=True, text=True)
    if out.returncode != 1 or not fx.transcript(live).exists():
        raise Failure(f"the CLI let a live session be archived: {out.returncode} {out.stderr}")
    lines = out.stderr.strip().splitlines()
    kind = json.loads(lines[-1]).get("error", {}).get("kind") if lines else None
    if out.stdout or kind != "session_live":
        raise Failure(f"the refusal is not an error object of kind session_live: {out.stderr!r}")


SCENARIOS: dict[str, Callable[[Run], None]] = {
    "open_and_keys": s_open_and_keys,
    "toggle_on_empty_view": s_toggle_on_empty_view,
    "moving_speed": s_moving_speed,
    "filter_search_escape": s_filter_search_escape,
    "detail": s_detail,
    "changes": s_changes,
    "export_and_copy": s_export_and_copy,
    "widths": s_widths,
    "mouse": s_mouse,
    "resume": s_resume,
    "guards": s_guards,
}
# Fixture size and layout per scenario, where not the default.
BULK = {"moving_speed": 3000}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("-k", default="", help="run scenarios whose name contains this")
    parser.add_argument("--mod", type=Path, default=REPO / "mod", help="mod folder to load")
    parser.add_argument("--cli", default=shutil.which("sessile") or "sessile", help="sessile binary")
    parser.add_argument("--keep", action="store_true", help="leave tmux and the fixture up after a failure")
    args = parser.parse_args()
    failed = []
    for name, scenario in SCENARIOS.items():
        if args.k not in name:
            continue
        root = Path(tempfile.mkdtemp(prefix=f"sessile-live-{name}-"))
        fx = build_fixture(root, default_sessions(BULK.get(name, 0)))
        began = time.time()
        run = None
        try:
            run = start(args, name, fx)
            scenario(run)
            print(f"PASS {name} ({time.time() - began:.0f} s)")
        except Exception as error:
            failed.append(name)
            detail = str(error) if isinstance(error, Failure) else traceback.format_exc()
            print(f"FAIL {name}: {detail}")
            if args.keep:
                print(f"  kept: tmux attach -t {run.screen.name if run else '?'}; fixture {root}")
                continue
        if run:
            run.screen.stop()
        shutil.rmtree(root, ignore_errors=True)
    print(f"\n{'FAILED: ' + ', '.join(failed) if failed else 'all passed'}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
