import SwiftUI

// How Trek moves (ios/docs/motion-audit.md): springs, so anything in motion can be caught and
// turned back; what comes and goes eases out; a press answers on touch-down; a drag follows the
// finger one to one and, let go, carries on at the finger's speed. Reduce Motion (iOS's or Trek's
// own, `MotionAllowed`) keeps the fades and drops the movement.

/// A press that answers as the finger lands: the label sinks a touch and dims, and comes back when
/// it lifts. In fast, out a little slower: the press is the user's, the release the app's. With
/// Reduce Motion it only dims.
struct PressStyle: ButtonStyle {
    /// How far it sinks. 0.97 for a card or a row; a small round button needs more to show at all
    /// (0.97 of 34 points is a point).
    var scale: CGFloat = 0.97
    var dim: Double = 0.8

    func makeBody(configuration: Configuration) -> some View {
        Pressed(label: configuration.label, pressed: configuration.isPressed, scale: scale, dim: dim)
    }

    private struct Pressed: View {
        let label: Configuration.Label
        var pressed: Bool
        var scale: CGFloat
        var dim: Double
        @MotionAllowed private var motion
        @Environment(\.isEnabled) private var enabled

        var body: some View {
            let down = pressed && enabled
            label
                .opacity(down ? dim : 1)
                .scaleEffect(down && motion ? scale : 1)
                .animation(down ? .snappy(duration: 0.1) : .snappy(duration: 0.22), value: down)
        }
    }
}

extension ButtonStyle where Self == PressStyle {
    /// A row, a card, a chip.
    static var press: PressStyle { PressStyle() }
    /// A small round button (send, the microphone, the jump to the latest).
    static var pressRound: PressStyle { PressStyle(scale: 0.88, dim: 0.85) }
}

/// The physics a drag hands over to when the finger lifts.
nonisolated enum Fling {
    /// Past an edge, the further it's pulled the less it follows (Apple's rubber band): `over`
    /// points past it, on something `size` points long, moves this far.
    static func rubberBand(_ over: CGFloat, size: CGFloat, constant: CGFloat = 0.55) -> CGFloat {
        guard size > 0 else { return 0 }
        let a = abs(over)
        return (a * size * constant / (size + constant * a)) * (over < 0 ? -1 : 1)
    }

    /// How much further a flick at `velocity` (points a second) carries before it comes to rest,
    /// as a scroll view slows (Apple's projection, `rate` 0.998 a millisecond).
    static func carry(_ velocity: CGFloat, rate: CGFloat = 0.998) -> CGFloat {
        velocity / 1000 * rate / (1 - rate)
    }

    /// A spring's starting speed relative to the way it has to go, which is what `UIView`'s and
    /// SwiftUI's springs take: `velocity` points a second, `distance` points to go. Capped, so a
    /// flick over the last few points doesn't fling it far past where it's going.
    static func relative(_ velocity: CGFloat, over distance: CGFloat, cap: CGFloat = 10) -> CGFloat {
        abs(distance) < 1 ? 0 : min(cap, max(-cap, velocity / distance))
    }
}

extension AnyTransition {
    /// Comes up from the bottom edge and fades in; goes the same way. Only a fade with Reduce Motion.
    static func rise(_ motion: Bool) -> AnyTransition {
        motion ? .move(edge: .bottom).combined(with: .opacity) : .opacity
    }

    /// A touch of blur while it fades, so what's fading and what's under it read as one change
    /// rather than two pictures on top of each other.
    static var soften: AnyTransition {
        .modifier(active: Soften(radius: 3), identity: Soften(radius: 0))
    }
}

private struct Soften: ViewModifier {
    var radius: CGFloat
    func body(content: Content) -> some View { content.blur(radius: radius) }
}
