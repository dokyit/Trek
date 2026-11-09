# Motion audit: Trek for iPhone

November 2026, at `1211f31`. An audit of how the iPhone app moves and answers touch, done the way
Emil Kowalski's design-engineering skills describe it (`improve-animations`, its `AUDIT.md`,
`find-animation-opportunities`, `review-animations` `STANDARDS.md`, `gestures`, `apple-design`,
`emil-design-eng`), translated from the web to SwiftUI and UIKit. Each finding says what's wrong,
which rule it breaks, the fix, and whether this pass made it.

Verified in the demo (`-TrekDemo YES`) on an iPhone 17 simulator (iOS 27), with before and after
recordings checked frame by frame.

## Recon

- **Stack.** SwiftUI on iOS 26+, with two pieces in UIKit: the transcript is a
  `UICollectionView` of hosted SwiftUI rows (`TranscriptCollection`, tuned with the scroll probe
  so the reader's place never jumps), and the image viewer zooms and pans in a `UIScrollView`.
  No animation library.
- **Conventions.** Springs throughout (`.snappy`, `.smooth`), mostly 0.15–0.3 s. Every endless
  animation is clock-driven (`TimelineView`: `BeaconDot`, `Breath`, `Sweep`, the hiker), never
  `repeatForever`: on iOS 27.2 a repeating animation leaked into the toolbar and the thread
  list and moved them by themselves. `@MotionAllowed` combines iOS's Reduce Motion with Trek's
  own Calm setting. A thread row animates only when it moves between sections, not on every
  activity update.
- **Personality.** A crisp tool, a companion to the Mac: quiet, fast, no bounce for its own sake.
- **Frequency map.**
  - 100+ a day: scrolling the transcript, typing, opening threads, going back.
  - Tens a day: sending, answering approvals, questions and plans, the tool groups, the jump to
    the latest, toasts ("Copied"), swipe actions.
  - Occasional: attachments, dictation, the image viewer, the new-thread sheet, settings,
    errors.
  - Rare: pairing, empty states, the update card.

## Findings

Ordered by leverage (impact ÷ effort). Severity follows `AUDIT.md`: HIGH breaks the feel,
MEDIUM is noticeably off, LOW is polish.

| # | Sev | Area | Where (at `1211f31`) | What's wrong | Rule | Fix | Done |
|---|---|---|---|---|---|---|---|
| 1 | HIGH | Request cards leaving | `ThreadView.swift` `ComposerArea` (cards at 838–866, `.animation` at 938); `AppModel.answer` | Tapping Allow, Deny, Answer or Approve changed nothing for a round trip to the Mac (about 0.5 s even in the demo; longer over Wi-Fi or Tailscale). Then the card slid down `.move(edge: .bottom)` *through* the settings bar and the composer, clipped along its right edge, and left a ghost of its title under the composer for a few frames (`before-approve-exit.png`). | apple-design §1 (respond on touch, any latency on the input path is a regression); §13 harmony (feedback on the frame of its cause); `gestures` (one entity, one timing family); `AUDIT.md` §3 physicality | The card goes as the finger lifts: kept back locally (`answered`), put back if the Mac refuses or the link drops (the toast says why), or if it's still asking 3 s after the Mac took the answer. It leaves where it stands: scale 0.96 from its foot, fade, a 3 pt blur so it dissolves into the "Allowed" row arriving under it. It arrives with a 24 pt rise and a fade from behind the composer, which now draws over it (`zIndex`). Fade only with Reduce Motion. | ✅ |
| 2 | HIGH | Haptics | whole app (only the model picker and charts had any) | Nothing is felt on the decisive actions: sending, stopping a turn, allowing or denying, answering, approving a plan, picking an option, an error. | apple-design §13 (multimodal feedback: causality, harmony, utility; feedback for completion and error) | `.sensoryFeedback`, fired with the visual change: send a light impact, Stop a medium one; Allow, Answer, Approve a medium impact, Deny and "Send back" a light one; an option ticked `.selection`; an error toast `.error`; starting a thread a light impact. Nothing for confirmations like "Copied" (utility). | ✅ |
| 3 | HIGH | Image viewer, pull to close | `MessageImages.swift` `ZoomableImage.Coordinator.swiped` (273–286) | Only downward travel is tracked, so pulling up hits a wall at 0. Nothing is linked to the pull: the black stays opaque and the image doesn't shrink. Release is decided by `down > 120 or v > 900` whatever the direction, so a pull that ends going back up still closes. The snap back is a fixed 0.2 s ease that ignores the finger's speed. Closing hands over to the cover's own slide at its own speed (a seam), a second finger can move the image, and catching it mid-snap-back jumps it to its place first. | `gestures` (track 1:1 then hand to a spring seeded with the velocity; dismiss on distance or velocity; linked values; rubber-band; interruptibility; multi-touch); apple-design §3, §5, §6 (project momentum, decide by velocity sign), §9 | Down follows the finger 1:1; up rubber-bands (Apple's formula). The image shrinks to 85 % by half the screen while the black behind fades to clear (the cover's background is now clear, so the thread shows through) and the buttons fade out, all from one `ViewerDrag` value that only the backdrop and buttons read. On release the resting point is projected (Apple's 0.998 deceleration) and the direction decides: it carries on off the screen on a spring at the finger's speed (then the cover closes without its own slide), or springs back seeded with the velocity and a 0.15 bounce. A grab mid-spring starts from the image's on-screen position. One finger only. With Reduce Motion it fades out, or settles back with no bounce. The full image now crossfades in over the blurred thumbnail (0.2 s ease-out) instead of replacing it at once. | ✅ |
| 4 | HIGH | Toasts | `TrekApp.swift` overlay (182–188), `AppModel.show` (1249–1255) | A toast drops in from the top but can't be sent back up, stays 3 s even when it's a two-line error, can't be held to finish reading, and an error isn't felt. | `gestures` spatial consistency (dismiss in the direction it entered, as Sonner does); `emil-design-eng` Sonner principles (pause the timer while it's being handled); apple-design §13 | `ToastOverlay`: a drag moves it 1:1 upward and rubber-bands downward. It goes on 28 pt of travel or a projected flick, if the finger was moving up, at the finger's speed; otherwise it springs back with a little bounce. A finger on it holds it, and letting go gives 2 s more. Errors stay 5 s and are felt (`.error`). VoiceOver gets a Dismiss action. | ✅ |
| 5 | MEDIUM | Press feedback | `Composer.swift` send/stop, mic, dictation keep/discard; jump to latest (`ThreadView.swift` 343); attachment tiles and their remove buttons; question options (`RequestCards.swift` 288); new-thread start button and chips | `.plain` buttons only dim on press. The most-pressed custom controls in the app (send above all) have no physical give. | `emil-design-eng` ("buttons must feel responsive": scale on press, 0.95–0.98, 100–160 ms); apple-design §1 (respond on touch-down) | `PressStyle`: scale and dim on touch-down in 0.1 s, back in 0.22 s (the press is the user's, the release the app's). 0.97 for rows, options and chips; 0.88 for the 34–38 pt round buttons, where 0.97 would move them by a point. Dim only with Reduce Motion. Glass buttons (`.glass`, `.glassProminent`, `.interactive()`) already have the system's press, so they're left as they are. | ✅ |
| 6 | MEDIUM | Transcript rows arriving | `TranscriptCollection.swift` `apply` (240–252) | A sent message, the first words of an answer and an answered request's row appear at full opacity in one frame, with the rows above jumping up under them (`before-send-sheet.png`). | `find-animation-opportunities` (teleporting state); `emil-design-eng` (preventing jarring changes; ease-out for what enters) | Rows new at the end of the transcript, with the reader at the latest, fade in (0.25 s, `cubic-bezier(0.23, 1, 0.32, 1)`) and rise 12 pt on a spring with no bounce. It's Core Animation on the cell's content layer only, so the layout, the measured heights and the reader's place are untouched. Never on opening a thread, a page of earlier rows, or an answer re-cut into parts. Fade only with Reduce Motion. | ✅ |
| 7 | MEDIUM | Reduce Motion above the composer | `ThreadView.swift` 843–902 | The background strip, queued strip, limit bar, cut-short bar, command picker and request cards all slid in with `.move(edge: .bottom)` whatever the setting. | `AUDIT.md` §6 (Reduce Motion means gentler, not none: keep the fade, drop the movement) | `.rise(motion)` and the request transition fade only with Reduce Motion or Trek's Calm. | ✅ |
| 8 | LOW | Attachment tiles | `AttachmentStrip.swift` 19–50 | Tiles added or removed only crossfade. | `AUDIT.md` §3 (never from nothing: scale 0.9–0.97 plus opacity) | Scale 0.9 plus a fade in and out, the others closing up on the same spring. Fade only with Reduce Motion. | ✅ |
| 9 | LOW | Counts | Needs you count, section counts, the pill's "3 working" and its hand count | Digits swap in one frame as threads move between sections. | `find-animation-opportunities` (state indication) | `.contentTransition(.numericText(value:))`. They change inside the list's existing `.snappy` transaction, so they roll with the rows. | ✅ |
| 10 | LOW | Tokens | about 40 hand-typed `.snappy(duration:)`, `.smooth(duration:)` and `.easeOut(duration:)` | Near-identical values typed everywhere. | `AUDIT.md` §7 (cohesion and tokens) | All of them are springs within budget, so rewriting them all would be churn for nothing felt. New code uses `Design/Motion.swift` (`PressStyle`, `Fling`, `.rise`, `.soften`). | left |
| 11 | LOW | Jump-to-latest entrance | `ThreadView.swift` 347 | `.scale(scale: 0.6)` plus opacity, where the rule says 0.9–0.97. | `AUDIT.md` §3 | A 38 pt button over 0.2 s doesn't read as coming from nothing, so it's taste only. | left |
| 12 | LOW | Tool group open | `Transcript.swift` 60; `TranscriptCollection.animateResize` | The SwiftUI content uses `.snappy(0.22)` while the UIKit row resize uses a 0.24 s ease-in-out: two timing families. | `gestures` (one entity, one timing family) | They're 20 ms apart and both soft, and the resize is tuned against the scroll probe. Not worth the risk. | left |

### What's already right (keep it)

- Clock-driven endless motion (`BeaconDot`, `Shimmer`, the hiker, `WorkingLine`), all off under
  `MotionAllowed`. The new code adds no endless animation, and nothing that runs in the toolbar
  or the thread list's rows by itself.
- Thread rows animate only on a section change, not on every activity tick (`AppModel.handle`,
  `.thread`): this follows the frequency rule.
- Words that change often swap with `.identity` instead of crossfading (the connection banner,
  the trail word).
- The update card fades out where it stands, fast (0.12 s), instead of sliding over the title.
- System-native physics stays native: sheets and their drag to dismiss, navigation pushes,
  swipe actions (with their own haptic at the full-swipe point), context menus, pull to refresh,
  `Menu`, glass buttons. None of it is reimplemented.
- Pickers and charts already give `.selection` haptics.

## Missed opportunities, gated

Each candidate went through the four gate questions in `find-animation-opportunities`
(frequency, purpose, speed, function).

Taken: #1's entrance (spatial: from behind the composer), #6 (preventing a jarring change), #8
and #9 (state indication), and the image crossfade in #3.

Rejected:

- **Zoom transition into a thread** (`navigationTransition(.zoom)` from its row). Rejected on
  frequency: core navigation, tens to hundreds a day, where the system push is the expectation.
  It would also need `matchedTransitionSource` on `List` rows, the surface that moved by itself
  on iOS 27.2.
- **Zoom transition for the image viewer** (from its thumbnail). It's the best long-term answer
  for spatial consistency, but there are five presenters, the thumbnails live inside UIKit-hosted
  cells, and the system's interactive dismiss would compete with the `UIScrollView`'s zoom and
  pan. Too risky for this pass; #3 gets the feel without it.
- **Animated scroll to new transcript rows** (iMessage-style). Rejected as risky: it would fight
  the measured-height anchoring that keeps the reader's place (see the comment at the top of
  `TranscriptCollection.swift`).
- **Stagger on opening a thread or the list.** Rejected on frequency: decoration on a 100+ a day
  surface.
- **`.scrollTransition` on rows.** Rejected on function: decoration on dense, functional text.
- **A haptic when a request arrives while the thread is open.** Rejected on causality: the user
  didn't cause it, and a turn can ask several in a row.
- **A bounce on the "Inbox zero" empty state.** Taste only, and it sits inside the list.

## How the fixes were checked

- Before and after recordings (`xcrun simctl io … recordVideo --codec h264`) of approving a
  command, sending, a toast pulled and flicked, and the image viewer pulled and flicked, read
  frame by frame (contact sheets at 40–60 fps). After the change, also: answering a question, a
  plan card arriving as its thread loads, the approve flow with Trek's Calm on (a fade only), and
  the Threads list left alone for 5 s (only the ticking clock of a working row changes).
- The first image-viewer build moved the `UIScrollView` that SwiftUI hands the representable,
  and the frames showed the image stuck while the black faded, then jittering as it left:
  SwiftUI puts that view's transform back whenever it lays out, and the backdrop now makes it lay
  out every frame. The pull now moves a scroll view inside a plain container.
- The app's unit tests on one simulator: 156 passed, none failed.
- Still to check on a device: what the haptics feel like (the simulator has no Taptic Engine),
  and the toast and image-viewer gestures under a real finger at 120 Hz (`gestures`' touch
  checklist).
