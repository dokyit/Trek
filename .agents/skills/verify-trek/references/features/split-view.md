# Two threads side by side

## What it does

Shows two threads in the main window, each a whole thread: its own transcript, composer, model and scroll position. One pane has the focus (a thin line on its header, its title in full) and typing goes to it. The route is the focused pane's thread, so the sidebar's selection, the title bar, the tab strip, the tools panel, the terminal dock, the palette and the window's shortcuts follow the focus. A draft, Settings or Basecamp takes the whole main area while the split waits behind it; a new thread sent from a draft takes the pane that had the focus. A thread is never in both panes: picking one already open focuses it.

## Reach it as a user

Right-click a thread in the sidebar and choose **Open beside**, drag a row onto the right half of the main area (over a split, onto the pane it should replace), or press ⌘\ and pick a thread (Ctrl+\ on Windows). With a split open ⌘\ moves the focus across, ⌘W or the pane's × closes the pane, the divider drags and double-clicking it evens the panes. The palette has **Open a thread beside this one**, **Focus the other pane** and **Close this pane**.

Each pane needs 340 points. With less room the split won't open and a toast says why; a window narrowed after it opened shows the focused pane alone, and ⌘\ reaches the other.

## Reach it with the CLI

```sh
./.agents/skills/verify-trek/scripts/trek-dev control rclick card-<thread id>
./.agents/skills/verify-trek/scripts/trek-dev control click thread-open-beside
./.agents/skills/verify-trek/scripts/trek-dev screenshot split
```

Ids: `split-view`, `split-pane-left`, `split-pane-right`, `split-divider`, `split-focus-left`, `split-focus-right`, `split-close-left`, `split-close-right`, `split-drop`, `split-drop-left`, `split-drop-right`, `thread-open-beside`.

## Code

`crates/trek-app/src/workspace/split.rs` (the model, `Scope::Beside`), `crates/trek-app/src/root/split.rs` (the panes as drawn), `command_palette.rs` (`toggle_beside`), `sidebar.rs` (`draggable`), `tests/split.rs`.

## Verify

Each pane keeps its own draft and attachments; streaming in one pane leaves the other's transcript undrawn; closing either pane gives the other the full width with its message; a thread that finishes in either pane stays read; the layout returns after a restart. A window that never opens a split looks and behaves as before.
