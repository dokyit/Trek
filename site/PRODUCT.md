# Product

<!-- impeccable:product-schema 1 -->

Written 2026-11-11 from the README, the app and what the owner has said about the site. Lines marked
(inferred) have not been confirmed by the owner in so many words.

## Platform

web

## Stack

Static HTML and CSS with a little plain JavaScript. No framework and no build step. Hosted on
Cloudflare Pages at trekdesktop.com.

## Users

Developers who already use coding agents (Claude Code, Codex, OpenCode, Cursor, Copilot, API and
local models) and have more than one piece of work going at a time. They arrive from GitHub, a post
or a search, at a desk, on a Mac or a Windows PC, and decide within a minute whether to download.
(inferred)

The first people the launch speaks to are iOS developers, because Trek can run the iOS Simulator and
a real iPhone for the agent. (inferred for the site; it is the owner's launch decision)

## Product Purpose

Trek is a free, open-source, native desktop app that runs coding agents side by side and puts
whatever needs the person first. The site succeeds when a first-time visitor understands that from
one screen and downloads the app.

## Positioning

- **One inbox.** Every thread from every agent sits in one sidebar. A thread that waits on an
  approval, a question or a plan goes to the top, with the reason in words. Finished work sits under
  it, unread until opened.
- **The agents you already use, side by side.** Trek is the place they run, not another agent.
- **Native.** A real Mac and Windows app, not a web page in a wrapper.
- **The iOS loop in one app.** The agent can build an app, run it in the Simulator or on a real
  iPhone, see the screen and tap through it.
- **The phone is the inbox and the remote.** The agents keep running on the computer.

## Operating Context

A developer starts several threads across projects, leaves them, and comes back when one needs an
answer. Trek shows that with a menu bar or tray icon, a Dock or taskbar badge, notifications, and on
the iPhone app.

## Capabilities and Constraints

- macOS 13 or later (Apple silicon and Intel) and Windows 11 (x64). The iPhone app needs iOS 26.
- The iPhone app is built from source for now; there is no App Store or TestFlight build. The site
  must not present it as something a visitor can install with a tap.
- The Mac build is not notarized yet, so the first launch needs one extra step, which the install
  section explains.
- Free. Apache-2.0. Source at github.com/dokyit/Trek.
- Words the product uses: Inbox, thread, project, approval, question, plan, Paper, Night, Basecamp,
  Notes.

## Brand Commitments

- The name Trek, and the line "Every agent. One trail."
- The mark: a mountain with a dotted trail and a summit flag (adopted 2026-11-12, replacing the
  cairn). The ember accent, and the two themes the app really has: Paper (light, warm) and Night
  (dusk).
- The pixel-art hiker from the app, and the dotted trail it walks. On the site he appears only in
  the story section, never in the first screen, and the mark is never used as scenery.
- The owner's standing requirements for the site: it should look official and professional; clean,
  modern and a little minimal; nothing that reads as AI-made (default typefaces, italic or bold
  lead-ins); nobody should be able to say it copies a competitor's site; few product pictures, each a
  real capture of the app shown large and sharp; real animation or none.
- Few words, in short plain sentences, claiming only what the README says. Decorative art is a
  photograph or nothing. No letter marks, and any mark is checked against
  competitors' and popular app icons before it is shown.
- Public contact: support@trekdesktop.com.
- No trackers and no analytics.

## Evidence on Hand

- Real captures of the app, taken from the isolated test app in its demo dress (believable names,
  staged work). Staged content is said to be staged.
- The app's own art: the hiker's frames (`crates/trek-mascot`) and the mark (`assets/brand`; the
  site's copies of it are drawn by `script/site/make_icons.py`). The first screen's picture is a
  licensed photograph, credited in `site/README.md`.
- There are no testimonials, customer logos, usage numbers, benchmarks or press. None may be
  invented, and the site should not be built to need them.

## Product Principles

1. What needs you comes first.
2. Trek is the place, the agents are the workers; it favours none of them.
3. The work stays on your computer; the phone is a remote.
4. Say what is so: what is staged, what is not ready, what the first launch asks of you.

## Accessibility & Inclusion

The page reads the same without JavaScript and with Reduce Motion on. Text meets 4.5:1.
