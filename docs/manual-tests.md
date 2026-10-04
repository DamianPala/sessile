# sessile: human test

Whether a person can learn the tool from the pane alone. Whether it works is checked by the live tests (`tests/live/`); this is about understanding.

Rules:

- Do not read `SPEC.md` or the README first. Only the pane teaches you.
- After each task note: did you know what to press, where you hesitated, what surprised you, what you expected instead.
- Run it in a real project with many sessions. Tasks 4, 5 and 11 change sessions: archive is reversible; for 11 make a throwaway first: `claude -p "sessile: delete me" --max-turns 1` in that project.

## Tasks

1. **Find a session where you worked on some topic.** Try a word that is in the conversation, not in the title. Did you find out that Enter searches the text?
2. **Name it** so you find it again next week.
3. **Pin a session you keep coming back to.** Did you notice where it went and what marks it?
4. **Clean up.** Find the junk sessions and archive a few of them.
5. **Undo a mistake.** Archive one session too many and get it back right away.
6. **Get a session back from the archive** that you archived earlier.
7. **Resume an old session**, first one of this project, then one of another project in a new terminal tab (paste what the pane gives you). Did you expect the first one to switch this very conversation?
8. **Hand an old session to the current Claude as a file** and ask it something about that session.
9. **Find a session from another project.**
10. **Delete the empty sessions** of this project.
11. **Delete one chosen session** (the throwaway). Did the dialog tell you enough to be sure it was the right one?

## Only a person can judge

Do these along the way:

- **Feel:** hold `↓` in a long list. Does the ring stop when you let go?
- **Mouse:** click a row (it only selects), click it again (it opens), click a button, scroll with the wheel, click the prompt and then the pane again. Do you always know which side has the keys? Does select-then-open feel natural, or do you reach for a double click?
- **Look:** in a fullscreen terminal the pane docks at the side. Do the colours match your terminal, and is everything readable in a narrow pane?
- **Two cursors:** hover and the keyboard ring. Did it confuse you?

## What the live tests cannot reach

`tests/live/` sees the screen of a terminal with no clipboard, no mouse and no eyes; these need a person once per release:

- **Clipboard:** `i` copies the id and `r` on another project's session its resume command; paste both somewhere real (task 7 covers `r`).
- **Inline pane:** in a terminal narrower than 110 columns, or not fullscreen, the pane opens above the prompt with about a third of the screen. Is that enough, and does `ctrl+x ↑` make it bigger?
- **Band layout:** set `layout` to `band` in `/config`, then `/sessile`: the list draws above the prompt, `ctrl+x tab` gives it the keys, and in the detail the `esc [ back ]` button is the way back.
- **macOS and Windows:** the whole test, once a build runs there.

## Questions at the end

- Which key did you guess wrong, and what did you press instead?
- What did you want to do and could not find?
- Which word or label in the pane was unclear?
