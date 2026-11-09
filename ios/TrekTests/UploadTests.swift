import Foundation
import Testing
@testable import Trek

/// Videos and files sent with a message (`uploads`): the requests as the Mac reads them
/// (`crates/trek-remote/tests/protocol.rs`), the pieces a file goes up in, and how a message's
/// files read in its bubble.
struct UploadTests {
    @Test func aPieceIsShapedAsTheMacReadsIt() throws {
        let part = ClientMessage.uploadPart(uploadId: "3F2504E0-4F89-11D3-9A0C-0305E82C3301", name: "IMG_0042.MOV",
                                            total: 10_485_760, offset: 4_194_304, data: "AAEC").json(id: "20")
        #expect(part["type"] as? String == "upload_part")
        #expect(part["id"] as? String == "20")
        #expect(part["upload_id"] as? String == "3F2504E0-4F89-11D3-9A0C-0305E82C3301")
        #expect(part["name"] as? String == "IMG_0042.MOV")
        #expect(part["total"] as? Int64 == 10_485_760)
        #expect(part["offset"] as? Int64 == 4_194_304)
        #expect(part["data"] as? String == "AAEC")
        // It goes as JSON whole.
        let data = try ClientMessage.uploadPart(uploadId: "u", name: "a", total: 3, offset: 0, data: "AAEC").encoded(id: "1")
        let back = try #require(try JSONSerialization.jsonObject(with: data) as? [String: Any])
        #expect(back["total"] as? Int == 3)
    }

    @Test func aMessageNamesItsFilesAndOnlyWhenItHasSome() {
        let send = ClientMessage.send(threadId: "t", text: "What happens at 0:12?", mode: nil, images: [], files: ["a", "b"]).json(id: nil)
        #expect(send["files"] as? [String] == ["a", "b"])
        let plain = ClientMessage.send(threadId: "t", text: "x", mode: nil, images: []).json(id: nil)
        #expect(plain["files"] == nil, "an older Mac never sees it")
        let started = ClientMessage.newThread(projectId: "p", agent: "codex", model: nil, text: "Summarise this", worktree: false,
                                              effort: nil, access: nil, plan: false, images: [], files: ["a"]).json(id: nil)
        #expect(started["type"] as? String == "new_thread")
        #expect(started["files"] as? [String] == ["a"])
        let consulted = ClientMessage.sendConsult(threadId: "t", text: "Go", mode: nil, images: [],
                                                  consult: ConsultRequest(with: ["codex/gpt-5.6-sol/high"], style: .advise, implement: false),
                                                  files: ["a"]).json(id: nil)
        #expect(consulted["files"] as? [String] == ["a"])
    }

    @Test func progressIsRead() throws {
        let reply = try ServerMessage.decode(JSONSerialization.data(withJSONObject: [
            "type": "upload_progress", "re": "20", "upload_id": "3f2504e0-4f89-11d3-9a0c-0305e82c3301",
            "received": 4_194_304, "total": 10_485_760,
        ]))
        guard case .uploadProgress(let re, let p) = reply else { Issue.record("not progress"); return }
        #expect(re == "20")
        #expect(p.received == 4_194_304 && p.total == 10_485_760 && !p.finished)
        let last = try ServerMessage.decode(JSONSerialization.data(withJSONObject: [
            "type": "upload_progress", "upload_id": "u", "received": 3, "total": 3, "done": true,
        ]))
        guard case .uploadProgress(_, let done) = last else { Issue.record("not progress"); return }
        #expect(done.finished)
        #expect(MacFeature(rawValue: "uploads") == .uploads)
    }

    @Test func aFileGoesUpInPiecesOfAtMostFourMebibytes() {
        #expect(ClientMessage.uploadPiece == 4 << 20)
        let pieces = UploadPieces.of(total: 10, size: 4)
        #expect(pieces.map(\.offset) == [0, 4, 8])
        #expect(pieces.map(\.length) == [4, 4, 2])
        #expect(UploadPieces.of(total: 8, size: 4).map(\.length) == [4, 4])
        let empty = UploadPieces.of(total: 0)
        #expect(empty.count == 1 && empty[0].offset == 0 && empty[0].length == 0, "one empty piece, so the Mac has it")
        // A video's worth: every byte once, in order, none over the most.
        let total: Int64 = 1_000_000_007
        let big = UploadPieces.of(total: total)
        #expect(big.count == 239)
        #expect(big.allSatisfy { $0.length <= ClientMessage.uploadPiece && $0.length > 0 })
        #expect(zip(big, big.dropFirst()).allSatisfy { $0.offset + Int64($0.length) == $1.offset })
        #expect(big.last.map { $0.offset + Int64($0.length) } == total)
    }

    @Test func piecesAreReadFromTheFileAsTheyGo() throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("upload-\(UUID().uuidString).bin")
        let bytes = Data((0..<10).map { UInt8($0) })
        try bytes.write(to: url)
        defer { try? FileManager.default.removeItem(at: url) }
        let reader = UploadReader(url: url)
        #expect(try reader.size() == 10)
        var joined = Data()
        for piece in UploadPieces.of(total: 10, size: 4) {
            let part = try reader.part(id: "u", name: "a.bin", total: 10, offset: piece.offset, length: piece.length)
            guard case .uploadPart(_, _, let total, let offset, let data) = part else { Issue.record("not a piece"); return }
            #expect(total == 10 && offset == piece.offset)
            joined.append(try #require(Data(base64Encoded: data)))
        }
        #expect(joined == bytes)
        // A file that got shorter meanwhile isn't sent as if it hadn't.
        #expect(throws: UploadError.self) { try reader.part(id: "u", name: "a.bin", total: 20, offset: 8, length: 4) }
    }

    @Test func aMessageEndingInFilesShowsThemByName() {
        let video = "/Users/me/Library/Application Support/dev.trek.Trek/uploads/iPhone 2026-11-08 at 10.15.30 Clip of the bug.mov"
        let pdf = "/Users/me/Library/Application Support/dev.trek.Trek/uploads/iPhone 2026-11-08 at 10.15.31 report.pdf"
        let split = MessageFiles.split("What breaks at 0:03? @\"\(video)\" @\"\(pdf)\"")
        #expect(split.text == "What breaks at 0:03?")
        #expect(split.files == [video, pdf])
        #expect(MessageFiles.displayName(video) == "Clip of the bug.mov")
        #expect(MessageFiles.displayName("/tmp/plain.txt") == "plain.txt")
        #expect(MessageFiles.displayName("/x/iPhone 2026-11-08 at 10.15.30 ") == "iPhone 2026-11-08 at 10.15.30 ")

        // The Mac composer's unquoted kind, and one with nothing typed before it.
        #expect(MessageFiles.split("see @/Users/me/a.rs").files == ["/Users/me/a.rs"])
        let alone = MessageFiles.split("@~/Desktop/notes.txt")
        #expect(alone.text.isEmpty && alone.files == ["~/Desktop/notes.txt"])

        // In the middle of a sentence it's part of the sentence; relative paths and addresses too.
        let inline = "Compare @/Users/me/a.rs with the old one"
        #expect(MessageFiles.split(inline).text == inline)
        #expect(MessageFiles.split(inline).files.isEmpty)
        #expect(MessageFiles.split("mail me@host.com").files.isEmpty)
        #expect(MessageFiles.split("look at @src/main.rs").files.isEmpty)
        // Only the run at the end: a mention before more words stays.
        let mixed = MessageFiles.split("@/a.txt then this @/b.txt")
        #expect(mixed.text == "@/a.txt then this" && mixed.files == ["/b.txt"])
        // An unclosed quote is left as it is.
        #expect(MessageFiles.split("broken @\"/a b").files.isEmpty)
    }

    @Test func attachmentsSentWithoutWordsAreSaidInWords() {
        #expect(MessageFiles.caption(photos: 1, videos: 0, files: 0) == "Here's a photo.")
        #expect(MessageFiles.caption(photos: 0, videos: 2, files: 0) == "Here are 2 videos.")
        #expect(MessageFiles.caption(photos: 0, videos: 0, files: 1) == "Here's a file.")
        #expect(MessageFiles.caption(photos: 1, videos: 1, files: 1) == "Here are 3 attachments.")
        #expect(PickedFile.clock(42) == "0:42")
        #expect(PickedFile.clock(3725) == "1:02:05")
    }
}
