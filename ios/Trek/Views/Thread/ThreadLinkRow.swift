import SwiftUI

/// A row linking another thread, as the Mac's transcript draws it (`thread_view.rs`, lineage
/// rows): for a thread this one's agent started or handed its work to, that thread's agent, title
/// and how it's doing (a dot that breathes while it works); for the thread this one came from, a
/// quiet line across the top. A tap opens it. Its title, agent and state are read from its row
/// when the phone has it, so they stay live; the item's are as the Mac last sent them.
struct ThreadLinkRow: View {
    var link: ThreadLink
    @Environment(AppModel.self) private var model
    @Environment(\.openThread) private var openThread

    var body: some View {
        let t = model.thread(link.threadId)
        let title = t?.title ?? link.title
        let opens = t != nil || link.open
        Button {
            if let openThread { openThread(link.threadId) } else { model.openRequest = link.threadId }
        } label: {
            if link.link.isOutgoing {
                outgoing(title: title, agent: t?.agent ?? link.agent, state: t.map(LinkedThreadState.init) ?? link.state, opens: opens)
            } else {
                incoming(title: title, opens: opens)
            }
        }
        .buttonStyle(.plain)
        .disabled(!opens)
        .accessibilityHint(opens ? "Opens that thread" : "That thread was archived or deleted")
    }

    /// A thread this one started or handed over to: who runs it and how it's doing.
    private func outgoing(title: String, agent: AgentRef?, state: LinkedThreadState?, opens: Bool) -> some View {
        HStack(spacing: 11) {
            ZStack(alignment: .bottomTrailing) {
                if let agent {
                    AgentGlyph(agent, size: 26)
                } else {
                    Image(systemName: link.link.symbol).font(.system(size: 17)).foregroundStyle(Trek.muted).frame(width: 26, height: 26)
                }
                if let state {
                    BeaconDot(color: state.color, pulses: state == .working, size: 9)
                        .padding(2)
                        .background(Trek.background, in: Circle())
                        .offset(x: 4, y: 4)
                }
            }
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(link.link.label).foregroundStyle(Trek.muted).fixedSize()
                    Text(title).fontWeight(.medium).lineLimit(1)
                }
                .font(.ui(.subheadline))
                if let state {
                    Text(state.label).font(.ui(.footnote)).foregroundStyle(Trek.muted).lineLimit(1)
                }
            }
            Spacer(minLength: 4)
            if opens {
                Image(systemName: "chevron.right").font(.ui(.caption).weight(.semibold)).foregroundStyle(Trek.muted)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(Trek.foreground.opacity(0.04), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Trek.border, lineWidth: 0.75))
        .contentShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel([link.link.label, title, state?.label].compactMap { $0 }.joined(separator: ", "))
    }

    /// Where this thread came from: a line across the top, like a handover's.
    private func incoming(title: String, opens: Bool) -> some View {
        HStack(spacing: 6) {
            Image(systemName: link.link.symbol)
            Text(link.link.label).fixedSize()
            Text(title).fontWeight(.medium).foregroundStyle(Trek.foreground.opacity(0.85)).lineLimit(1)
            if opens { Image(systemName: "chevron.right").font(.ui(.caption2).weight(.semibold)) }
        }
        .font(.ui(.footnote))
        .foregroundStyle(Trek.muted)
        .padding(.horizontal, 12)
        .padding(.vertical, 7)
        .background(Trek.foreground.opacity(0.04), in: Capsule())
        .contentShape(Capsule())
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(link.link.label) \(title)")
    }
}

/// Over a thread another thread started or took over from, while the transcript's own row for
/// that isn't loaded (it's the first item, and only the latest page comes at first): the same
/// row, from the thread's `startedBy`/`continuedFrom`. Observes the thread's row and blocks
/// itself, so the transcript doesn't redraw for it.
struct LineageHeader: View {
    let store: ThreadStore
    @Environment(AppModel.self) private var model

    var body: some View {
        if store.loaded, let link {
            ThreadLinkRow(link: link)
        }
    }

    private var link: ThreadLink? {
        guard let t = store.summary else { return nil }
        let origin: (ThreadLinkKind, String)? = if let id = t.startedBy { (.startedBy, id) } else if let id = t.continuedFrom { (.continuedFrom, id) } else { nil }
        guard let (kind, id) = origin else { return nil }
        let shown = store.blocks.contains { b in
            if case .threadLink(let l) = b.kind { l.threadId == id && !l.link.isOutgoing } else { false }
        }
        guard !shown else { return nil }
        return ThreadLink(link: kind, threadId: id, title: model.thread(id)?.title ?? "another thread")
    }
}
