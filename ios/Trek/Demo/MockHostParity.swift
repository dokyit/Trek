import Foundation

/// The demo Mac answering what the phone gained to keep up with the Mac (see `DemoParity`).
extension MockHost {
    func takeParity(_ message: ClientMessage, id: String?) {
        switch message {
        case .useReset(let creditId):
            guard DemoMac.resetCredits.contains(where: { $0.id == creditId }) else {
                return deliver(.error(re: id, code: .notFound, message: "That reset isn't available any more"))
            }
            later(0.8) {
                DemoMac.resetCredits.removeAll { $0.id == creditId }
                self.ack(id)
            }
        case .queueAction(let tid, let index, let reset, let text, let edit):
            var taken: String?
            var gone = true
            update(tid) { t in
                var queued = t.queued ?? []
                let places = queued.indices.filter { (queued[$0].reset ?? false) == reset }
                guard index < places.count, queued[places[index]].text == text else { return }
                gone = false
                if let edit {
                    queued[places[index]].text = edit
                } else {
                    taken = queued.remove(at: places[index]).text
                }
                t.queued = queued.isEmpty ? nil : queued
            }
            if gone { return deliver(.error(re: id, code: .conflict, message: "That message isn't queued any more")) }
            deliver(.ack(re: id, threadId: nil, text: taken))
        case .attachment(let tid, let itemId, let index, let maxPx):
            guard let (data, w, h) = DemoMac.attachment(index, maxPx: min(maxPx ?? 2048, 2048)) else { return }
            let a = Attachment(threadId: tid, itemId: itemId, index: index, mime: "image/jpeg", data: data.base64EncodedString(), width: w, height: h)
            deliver(.attachment(re: id, a), after: 0.25)
        case .backgroundOutput(let tid, let taskId):
            DemoMac.backgroundReads += 1
            deliver(.backgroundOutput(re: id, BackgroundOutput(threadId: tid, taskId: taskId,
                                                               output: DemoMac.backgroundOutput(taskId, tick: DemoMac.backgroundReads))), after: 0.2)
        case .stopBackground(let tid, let taskId):
            update(tid) { t in
                if let i = t.backgroundTasks?.firstIndex(where: { $0.id == taskId }) { t.backgroundTasks?[i].stopping = true }
            }
            ack(id)
            later(1.2) {
                self.update(tid) { t in
                    t.backgroundTasks?.removeAll { $0.id == taskId }
                    t.background = t.backgroundTasks?.map(\.title)
                    if t.backgroundTasks?.isEmpty == true { t.backgroundTasks = nil; t.background = nil }
                }
            }
        case .icon(let agent, let projectId):
            let hash = agent.flatMap { key in DemoMac.addedAgents.first { $0.key == key }?.icon } ?? projectId.flatMap(DemoMac.projectIcon)
            guard let hash, let (png, template) = DemoMac.icon(hash) else {
                return deliver(.error(re: id, code: .notFound, message: "No icon for that"))
            }
            deliver(.icon(re: id, IconReply(agent: agent, projectId: projectId, hash: hash, png: png.base64EncodedString(), template: template)))
        default:
            break
        }
    }
}
