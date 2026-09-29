# modelcmp keys

Every key and mouse action in the TUI. Press `?` in the TUI for the short list; the
[README](README.md) covers install and the basics, [REFERENCE.md](REFERENCE.md) the columns,
commands and JSON.

## Move

| Key | Action |
|-----|--------|
| `j` `k` | move; a count repeats, as in `3j`; past the last row back to the first |
| `h` `l` | pick a column, scrolling the ones right of Dev; `‹` `›` mark columns off screen, `▲` `▼` on the left border rows above or below; in compare and recommend, pick a model for `o` `x` `y` `f` `e` `n`, with the same marks for models off screen |
| `0` `_` `$` `w` `b` | first / last column; next / previous group: prices, benchmarks, Via; in compare and recommend, `0` `_` `$` pick the first / last model |
| `gg` `G` `3gg` | top / bottom / row 3; a number before `gg` goes to that row, as `12gg` to row 12, in dropdowns and lists too |
| `(` `)` `^d` `^u` | half a page up / down; overlays and dropdowns mark lines off screen with the same `▲` `▼` |

## Filter and sort

| Key | Action |
|-----|--------|
| `s` | sort by the column; again reverses |
| `/` | filter by name, developer, Via or note, words in any order (`anthropic opus`), a typo forgiven when nothing matches (`opsu`); `/` again starts a new search, `esc` clears; in compare it filters the rows, in the help its lines, and in a list like the theme panel its entries; what matched is underlined in yellow everywhere |
| `>` `<` | minimum / maximum for the column, e.g. `>` `70` `enter` on Coding; Ctx in thousands of tokens, `>` `200` for 200k |
| `d` | dropdown on the Dev, Price and Via headers (▾); `/` searches it, as in every list, `m` toggles several, as it selects models |
| `a` | all models, including ones you have no access to |
| `%` | Price with no input cached, as a one-off prompt, instead of the share it started with (an agent's 90%, or `--cache`); again: back to that share. The top border on Price says which, and the hint shows on the price columns, or anywhere while no input is cached. Code/$, the cheapest provider and recommend follow it. `modelcmp --cache 50` starts the TUI with any other share |
| `M` | selected models only; `M` again or `esc`: every model |
| `F` | favorite models only, the rows with a `★` (the picked task's favorite when a task is picked); `F` again or `esc`: every model |
| `E` | excluded models only, the rows with a red `✗`; `E` again or `esc`: every model |
| `c` | clear filters, bounds, task, `M`, `F` and `E`; the selection stays |

## Select and compare

| Key | Action |
|-----|--------|
| `m` | select the model: its box `☐` becomes `✓` in light blue; the shortlist you are deciding between, kept until you deselect it or close the TUI. A click on the box toggles it |
| `U` | deselect every model |
| `V` | highlight a range of rows: move to extend it, then `m` `e` or `C` act on all of it; `esc` cancels |
| `C` | compare 2+ selected models: cheapest, best coder, most coding per $; `h` `l` pick a model, and the table's bar follows it; `C` again or `esc` closes it |

## Model under the cursor

| Key | Action |
|-----|--------|
| `enter` | details: every benchmark, price per provider |
| `f` | favorite the model for tasks. Every row has a `☆`, filled bold `★` for a favorite: with a task picked, that task's favorite in the task's colour, the colour of its name in recommend; with none, a gold `★` for the favorite of any task. `f` or a click on the `☆` lists the tasks with `☐`/`✓`: `m` or a click ticks one, `enter` ticks the one under the bar and closes; the list starts on the picked task, or the task under the cursor in recommend. The status bar names the tasks the model under the cursor is the favorite for. The model joins the task's line even off the price frontier, greyed and marked `not recommended` there, and `--tier` picks it |
| `e` | exclude the model: you have it but cannot use it. It stays in the table, greyed out, but recommendations (`R` and `--task`) skip it. Every row has a `·`, a red `✗` when excluded, and a click on it toggles the exclusion of that row alone, even on a selected one |
| `e` on a selected model | act on every selected model, not just the one under the cursor |
| `n` | note for the model |
| `y` `Y` | copy the model id (`provider/model`) / the model name |
| `o` | open the model on models.dev, epoch.ai, artificialanalysis.ai or openrouter.ai; asks which |
| `x` | open a harness on the model in a new terminal (Windows Terminal under WSL, else `$TERMINAL`); asks which when several have it |

## Panels

| Key | Action |
|-----|--------|
| `R` | recommend: the best model per price for each task, what it measures and when to use it; `h` `l` pick a model on the task's line for `o` `x` `y` `f` `e` `n`, and the table's bar follows it, so `esc` lands on it; `enter` shows the task's models in the table, best first, each row cheaper and scoring lower |
| `t` | theme panel: `j` `k` preview, `/` searches, `enter` saves, `esc` or `t` closes. `terminal` (its own colours, the default), then the dark `gruvbox`, `nord`, `catppuccin`, `dracula`, `tokyonight`, `kanagawa`, `monokai`, `rose-pine`, `github`, `solarized`, `synthwave` and `cyberpunk`, the light `github-light`, `gruvbox-light`, `paper` and `sepia`, and the retro `amber`, `phosphor`, `c64` and `gameboy`; saved with your selection. A theme sets the 16 terminal colours, the text colour and 5 to 14 colours for developers, each of which has to read on that theme's background and stay apart from the others. Every theme paints its own background, so a dark one stays dark on a light terminal; only `terminal` keeps a transparent background |
| `?` | help; `/` keeps the lines that match |

## Input and mouse

| Key | Action |
|-----|--------|
| typing | `←` `→` `^a` `^e` move, `alt-b` `alt-f` `^←` `^→` by word; `^w` `alt-d` delete a word, `^u` `^k` to the start / end |
| mouse | click a row to highlight it, again for details; ctrl click adds or removes it from the highlight, shift click or a drag highlights a range, a plain click drops the highlight, right click selects it, a click on its `☐` toggles the selection, on its `☆` picks its tasks, on its `✗` box excludes it; a click on the `#` header goes to the first row, on the `✓` header shows selected models only, on the `★` header favorites only, on the `✗` header excluded only; a header sorts, its ▾ opens the dropdown, where clicks toggle entries until a click elsewhere; a click on a key hint in the status bar presses that key; the wheel scrolls, sideways moves the column, or the model in compare and recommend |

## General

| Key | Action |
|-----|--------|
| `r` | refresh data now |
| `qq` | quit; the first `q` asks. `esc` goes back: closes an overlay, drops the highlight, clears the `/` filter, leaves `M`, then `F`, then `E`, then a task picked in recommend back to recommend |
