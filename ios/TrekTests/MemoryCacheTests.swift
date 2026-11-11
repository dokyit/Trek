import Testing
import UIKit
@testable import Trek

/// Caches kept by what they hold, not by how many things (`ByteLRU`).
struct ByteLRUTests {
    @Test func theLeastRecentlyUsedGoFirstAndOnlyAsManyAsItTakes() {
        var lru = ByteLRU<String, Int>(budget: 100, entryCap: 60)
        let (first, second) = (lru.insert(1, for: "a", cost: 40), lru.insert(2, for: "b", cost: 40))
        #expect(first && second)
        #expect(lru.count == 2 && lru.cost == 80, "80 of 100: both fit")
        // 120 of 100: the oldest goes, and that's enough.
        lru.insert(3, for: "c", cost: 40)
        #expect(!lru.contains("a") && lru.contains("b") && lru.contains("c"))
        #expect(lru.cost == 80)
        lru.insert(4, for: "d", cost: 40)
        #expect(!lru.contains("b") && lru.contains("c") && lru.contains("d"))
    }

    @Test func aHitMakesAnEntryTheMostRecent() {
        var lru = ByteLRU<String, Int>(budget: 100, entryCap: 60)
        lru.insert(1, for: "a", cost: 40)
        lru.insert(2, for: "b", cost: 40)
        // Read: `a` is now newer than `b`, which goes in its place. Looked for only: no use.
        let hit = lru.value(for: "a")
        #expect(hit == 1)
        #expect(lru.contains("b"))
        lru.insert(3, for: "c", cost: 40)
        #expect(lru.contains("a") && !lru.contains("b") && lru.contains("c"))
        let miss = lru.value(for: "b")
        #expect(miss == nil)
    }

    @Test func oneEntryOverTheCapIsNotKeptAndTakesNothingWithIt() {
        var lru = ByteLRU<String, Int>(budget: 100, entryCap: 30)
        lru.insert(1, for: "small", cost: 20)
        lru.insert(2, for: "also small", cost: 20)
        // 90 where 30 is the most for one: not kept, though whoever made it has it to use.
        let kept = lru.insert(3, for: "huge", cost: 90)
        #expect(!kept && !lru.contains("huge"))
        #expect(lru.count == 2 && lru.cost == 40, "and the small ones are still there")
        // One that grew past the cap goes, its smaller self with it.
        lru.insert(4, for: "grows", cost: 10)
        let grown = lru.insert(5, for: "grows", cost: 50)
        #expect(!grown && !lru.contains("grows") && lru.cost == 40)
    }

    @Test func replacingAnEntryCountsItsNewCostOnce() {
        var lru = ByteLRU<String, Int>(budget: 100, entryCap: 60)
        lru.insert(1, for: "a", cost: 40)
        lru.insert(2, for: "a", cost: 10)
        let now = lru.value(for: "a")
        #expect(lru.count == 1 && lru.cost == 10 && now == 2)
        lru.remove("a")
        #expect(lru.count == 0 && lru.cost == 0)
    }

    @Test func aStreamingTextReplacesItsShorterSelf() {
        var lru = ByteLRU<String, Int>(budget: 1 << 20, entryCap: 1 << 16) { new, old in new.extends(old) }
        // A finished answer, then one that streams in a word at a time, as many versions.
        lru.insert(-1, for: "An answer from before.", cost: 22)
        var text = ""
        for word in 0..<500 {
            text += "word \(word) "
            lru.insert(word, for: text, cost: text.utf8.count)
        }
        #expect(lru.count == 2, "the finished answer, and the latest of the 500")
        let latest = lru.value(for: text)
        #expect(latest == 499)
        #expect(lru.cost == 22 + text.utf8.count)
        #expect(lru.contains("An answer from before."))
        // Two streaming at once, taking turns (a thought and an answer): one entry each.
        var (thought, answer) = ("Thinking: ", "Answer: ")
        for n in 0..<100 {
            thought += "t\(n) "
            lru.insert(n, for: thought, cost: thought.utf8.count)
            answer += "a\(n) "
            lru.insert(n, for: answer, cost: answer.utf8.count)
        }
        #expect(lru.count == 4)
        #expect(lru.contains(thought) && lru.contains(answer))
        // Asked for again unchanged, a text stays (it doesn't supersede itself).
        lru.insert(7, for: answer, cost: answer.utf8.count)
        let again = lru.value(for: answer)
        #expect(again == 7 && lru.count == 4)
    }

    @Test func aStreamedAnswerLeavesOneParsedCopy() {
        // As the transcript asks while an answer streams in: the text so far, each time longer.
        let tag = UUID().uuidString
        var text = "## Answer \(tag)\n\n"
        var versions: [String] = []
        for n in 0..<120 {
            text += "Sentence \(n) of the answer. "
            versions.append(text)
            #expect(!Markdown.blocks(text).isEmpty)
        }
        // Other tests parse at the same time, and one of theirs can come between two versions:
        // a few of the 119 shorter ones may be left, never most of them as before.
        #expect(Markdown.isCached(text))
        #expect(versions.dropLast().filter(Markdown.isCached).count <= 5, "the shorter versions aren't kept")
        // The same for the line of it that's growing.
        var line = "Line \(tag): "
        var lines: [String] = []
        for n in 0..<120 {
            line += "`code\(n)` and "
            lines.append(line)
            _ = Inline.parsed(line)
        }
        #expect(Inline.isParsedCached(line))
        #expect(lines.dropLast().filter(Inline.isParsedCached).count <= 5)
    }

    @Test func anAnswerTooLongToKeepIsStillParsed() {
        let long = "# \(UUID().uuidString)\n\n" + String(repeating: "A long line of the answer.\n\n", count: Markdown.cacheEntryCap / 40)
        #expect(Markdown.cost(long) > Markdown.cacheEntryCap)
        let blocks = Markdown.blocks(long)
        #expect(blocks.count > 1000, "parsed all the same")
        #expect(!Markdown.isCached(long), "and not kept")
        #expect(Markdown.blocks(long) == blocks)
    }
}

/// The image cache counts pixels, whatever its callers say a picture costs.
@MainActor
struct DecodedImageCacheTests {
    private func picture(_ width: Int, _ height: Int) -> UIImage {
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let size = CGSize(width: width, height: height)
        return UIGraphicsImageRenderer(size: size, format: format).image { context in
            UIColor.systemOrange.setFill()
            context.fill(CGRect(origin: .zero, size: size))
        }
    }

    @Test func aPictureCostsItsPixelsNotItsFile() throws {
        let shot = picture(1000, 800)
        let cost = DecodedImageCache.decodedCost(shot)
        #expect(cost >= 1000 * 800 * 4)
        // What callers pass as its cost: the file's size, a small part of that.
        let file = try #require(shot.jpegData(compressionQuality: 0.8)).count
        #expect(file * 10 < cost)
    }

    @Test func aPictureOverTheCapIsNotKeptWhateverCostItIsGiven() {
        let cache = DecodedImageCache(budget: 8 << 20, entryCap: 1 << 20)
        cache.setObject(picture(100, 100), forKey: "small", cost: 5)
        // 3.2 MB of pixels where 1 MiB is the most for one, passed off as 5 bytes.
        cache.setObject(picture(1000, 800), forKey: "big", cost: 5)
        cache.setObject(picture(1000, 800), forKey: "big, no cost")
        #expect(cache.object(forKey: "small") != nil)
        #expect(cache.object(forKey: "big") == nil)
        #expect(cache.object(forKey: "big, no cost") == nil)
        // A key's smaller picture doesn't outlive a larger one that isn't kept.
        cache.setObject(picture(1000, 800), forKey: "small")
        #expect(cache.object(forKey: "small") == nil)
    }

    @Test func aPhotoIsKeptAtTheViewersSize() {
        // A photo off the camera: 48 MB whole.
        let photo = picture(4032, 3024)
        #expect(DecodedImageCache.decodedCost(photo) > Attachments.entryCap)
        let fitted = Attachments.fitted(photo, maxPx: 2048)
        #expect(fitted.size == CGSize(width: 2048, height: 1536))
        #expect(DecodedImageCache.decodedCost(fitted) <= Attachments.entryCap)
        // A small one is kept as it is.
        let small = picture(300, 200)
        #expect(Attachments.fitted(small, maxPx: 2048) === small)
    }
}
