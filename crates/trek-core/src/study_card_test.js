// Runs the question card's script (`study_card.html`, its path the first argument) in node with a
// tiny stand-in DOM and checks what the learner sees: an answer sent before is shown as it was
// sent, read-only; dictation fills the thinking box as it's heard; answering keeps it all.
// `study::tests::the_card_keeps_its_answer_and_takes_dictation` runs it.
const fs = require('fs');
const vm = require('vm');
const assert = require('assert');
const html = fs.readFileSync(process.argv[2], 'utf8');
const script = html.slice(html.lastIndexOf('<script>') + 8, html.lastIndexOf('</script>'));

class El {
  constructor(tag) {
    Object.assign(this, { tag, children: [], className: '', textContent: '', style: {}, disabled: false, value: '', readOnly: false, parent: null });
    const has = () => this.className.split(' ').filter(Boolean);
    this.classList = {
      add: (...c) => { this.className = [...new Set([...has(), ...c])].join(' '); },
      remove: (...c) => { this.className = has().filter((x) => !c.includes(x)).join(' '); },
      toggle: (c, on) => (on ? this.classList.add(c) : this.classList.remove(c)),
      contains: (c) => has().includes(c),
    };
  }
  append(...k) { k.forEach((x) => { x.parent = this; this.children.push(x); }); }
  replaceWith(x) { const p = this.parent; p.children[p.children.indexOf(this)] = x; x.parent = p; }
  setAttribute() {}
  set innerHTML(_) {}
  get scrollHeight() { return 80; }
  focus() {}
  all(pred, out = []) { this.children.forEach((c) => { if (pred(c)) out.push(c); c.all(pred, out); }); return out; }
  querySelectorAll() { return this.all((e) => e.tag === 'button'); }
  text() { return this.textContent + this.children.map((c) => c.text()).join(''); }
}

function run(data, replies, kept = null) {
  const card = new El('div');
  const listeners = {}, sent = [], dictated = [], keeps = [], timers = [];
  const window = {
    trek: { send: (t) => (sent.push(t), true), copy: () => true, replies, canDictate: true, dictate: (on) => (dictated.push(on), true),
      kept, keep: (t) => (keeps.push(t), true) },
    addEventListener: (t, f) => { (listeners[t] = listeners[t] || []).push(f); },
  };
  const document = {
    createElement: (t) => new El(t),
    createTextNode: (t) => Object.assign(new El('#text'), { textContent: t }),
    getElementById: () => card,
    addEventListener: () => {},
  };
  vm.runInContext(script.replace('__DATA__', JSON.stringify(data)), vm.createContext({ window, document, navigator: { platform: 'MacIntel' }, setTimeout: (f) => timers.push(f) }));
  const cls = (c) => (e) => e.classList.contains(c);
  return {
    card, sent, dictated, keeps,
    // Run what the card put off (its debounced `trek.keep`).
    tick: () => { while (timers.length) timers.shift()(); },
    hear: (detail) => (listeners.trekdictation || []).forEach((f) => f({ detail })),
    box: card.all((e) => e.tag === 'textarea')[0],
    mic: card.all(cls('mic'))[0],
    options: card.all(cls('opt')),
    sure: card.all((e) => e.parent && e.parent.classList.contains('sure')),
    main: card.all(cls('go'))[0],
    line: () => (card.all(cls('sent'))[0] || { text: () => null }).text(),
    note: () => card.all(cls('note'))[0].textContent,
    kept: () => card.all(cls('kept'))[0],
  };
}
const on = (list) => list.map((e) => e.classList.contains('on'));

const question = { kind: 'question', step: '', prompt: 'What sets growth?', options: ['a', 'b', 'Population growth $\\bar n$', 'd'], targets: [] };
const answer = "Question\nAnswer: C. Population growth $\\bar n$\nConfidence: Unsure (45%)\nWorking: I honestly\ndon't remember";

// Answered before: the choice, how sure and the thinking stay, read-only, with what was sent.
let c = run(question, ['typed in the composer', answer]);
assert.ok(c.card.classList.contains('done'));
assert.deepStrictEqual(on(c.options), [false, false, true, false]);
assert.deepStrictEqual(on(c.sure), [false, true, false, false]);
assert.strictEqual(c.box.value, "I honestly\ndon't remember");
assert.ok(c.box.readOnly && c.mic.disabled);
// Kept as it reads, not as a box: the lines as one paragraph, the box hidden.
assert.ok(c.box.hidden);
assert.strictEqual(c.kept().text(), "I honestlydon't remember");
assert.deepStrictEqual(c.kept().children.map((e) => e.tag), ['p']);
assert.deepStrictEqual(c.kept().children[0].children.map((e) => e.tag), ['#text', 'br', '#text']);
assert.strictEqual(c.line(), 'Sent · C. Population growth $\\bar n$ · Unsure (45%)');
// "I don't know": closed, nothing chosen.
c = run(question, ["Question\nAnswer: I don't know\nWorking: (none)"]);
assert.ok(c.card.classList.contains('done'));
assert.deepStrictEqual(on(c.options), [false, false, false, false]);
assert.strictEqual(c.box.value, '');
assert.ok(!c.kept(), 'nothing written, nothing kept');
assert.ok(c.box.parent.parent.hidden, 'and no empty box');
// Another card's answer (another head) isn't this one's.
c = run(question, [answer.replace('Question', 'Check · Step 1 of 4')]);
assert.ok(!c.card.classList.contains('done'));

// Dictation: what's heard so far shows after what was typed, is revised, and stays when done.
c = run(question, []);
assert.ok(c.mic && !c.mic.disabled);
c.box.value = 'I think';
c.mic.onclick();
assert.deepStrictEqual(c.dictated, [true]);
c.hear({ state: 'asking', text: '' });
c.hear({ state: 'listening', text: 'it is' });
assert.strictEqual(c.box.value, 'I think it is');
assert.ok(c.mic.classList.contains('on'));
c.hear({ state: 'listening', text: 'it is population' });
assert.strictEqual(c.box.value, 'I think it is population');
c.mic.onclick();
assert.deepStrictEqual(c.dictated, [true, false], 'the mic again ends the take');
c.hear({ state: 'finishing', text: 'it is population' });
c.hear({ state: 'done', text: 'it is population growth' });
assert.strictEqual(c.box.value, 'I think it is population growth');
assert.ok(!c.mic.classList.contains('on'));
// A take that ends with nothing leaves the box as it was, and says why.
c.hear({ state: 'asking' });
c.hear({ state: 'idle', error: 'Nothing heard.' });
assert.strictEqual(c.box.value, 'I think it is population growth');
assert.strictEqual(c.note(), 'Nothing heard.');

// Answering sends it all and keeps it on the card.
c.options[2].onclick();
c.sure[1].onclick();
c.main.onclick();
assert.deepStrictEqual(c.sent, ['Question\nAnswer: C. Population growth $\\bar n$\nConfidence: Unsure (45%)\nWorking: I think it is population growth']);
assert.ok(c.card.classList.contains('done') && c.box.readOnly && c.box.hidden);
assert.strictEqual(c.kept().text(), 'I think it is population growth');
assert.deepStrictEqual(on(c.options), [false, false, true, false]);
assert.deepStrictEqual(on(c.sure), [false, true, false, false]);
assert.strictEqual(c.box.value, 'I think it is population growth');
// Heard after it closed: nothing changes.
c.hear({ state: 'done', text: 'late' });
assert.strictEqual(c.box.value, 'I think it is population growth');

// Maths and Markdown in the thinking: the maths stays text, whole and with its delimiters, for
// KaTeX to set (a `*` or `_` in it is never emphasis); bold, italic, code and lists are elements.
const thinking = 'So $f(x)=x^2$ and **not** $a_1 * b_2$, `x*y` _maybe_\n\n- first $$\\int_0^1 f$$\n- second *step*\n1. one';
c = run(question, [`Question\nAnswer: I don't know\nWorking: ${thinking}`]);
const k = c.kept();
const tags = (e) => e.children.map((x) => x.tag);
assert.deepStrictEqual(tags(k), ['p', 'ul', 'ol']);
const [p, ul, ol] = k.children;
assert.deepStrictEqual(tags(p), ['#text', '#text', '#text', 'strong', '#text', '#text', '#text', 'code', '#text', 'em']);
assert.deepStrictEqual(p.children.filter((e) => e.tag === '#text').map((e) => e.textContent),
  ['So ', '$f(x)=x^2$', ' and ', ' ', '$a_1 * b_2$', ', ', ' ']);
assert.strictEqual(p.children[3].text(), 'not');
assert.strictEqual(p.children[7].text(), 'x*y');
assert.strictEqual(p.children[9].text(), 'maybe');
assert.deepStrictEqual(ul.children.map((li) => li.text()), ['first $$\\int_0^1 f$$', 'second step']);
assert.deepStrictEqual(tags(ul.children[1]), ['#text', 'em']);
assert.deepStrictEqual(ol.children.map((li) => li.text()), ['one']);
// Nothing of it is ever HTML: markup typed in it is text.
c = run(question, ['Question\nAnswer: I don\'t know\nWorking: <img src=x onerror=alert(1)>']);
assert.strictEqual(c.kept().text(), '<img src=x onerror=alert(1)>');

// The question and its options read as written, Markdown set (a tutor's `**minimum**` is bold,
// not asterisks), a code block kept as code, the maths left for KaTeX.
const marked = { kind: 'question', step: '', prompt: 'What is the **minimum** of $f(x)$?\n```\nlet r = f [3; -1]\n```', options: ['`2`', 'the *least* $f(x)$'], targets: [] };
c = run(marked, []);
const q = c.card.all((e) => e.classList.contains('q'))[0];
assert.deepStrictEqual(tags(q), ['p', 'pre']);
assert.deepStrictEqual(tags(q.children[0]), ['#text', 'strong', '#text', '#text', '#text']);
assert.strictEqual(q.children[0].text(), 'What is the minimum of $f(x)$?');
assert.strictEqual(q.children[1].text(), 'let r = f [3; -1]');
assert.strictEqual(c.options[0].text(), 'A2');
assert.deepStrictEqual(tags(c.options[1].children[1]), ['#text', 'em', '#text', '#text']);

// A problem stays open for more working until it's solved.
const problem = { kind: 'problem', step: '', prompt: 'x', options: [], targets: ['c2'] };
c = run(problem, ['Problem [c2]\nStatus: still working\nWorking: abc']);
assert.ok(!c.card.classList.contains('done'));
assert.strictEqual(c.note(), 'Sent · keep going');
c = run(problem, ['Problem [c2]\nStatus: still working\nWorking: abc', "Problem [c2]\nStatus: I think I've solved it\nWorking: got it"]);
assert.ok(c.card.classList.contains('done'));
assert.strictEqual(c.box.value, 'got it');

// What's chosen and typed is handed to Trek as it changes, and a card drawn again before it was
// answered (its page let go while it was scrolled away) opens with it all still there.
c = run(question, []);
c.options[1].onclick();
c.sure[3].onclick();
c.box.value = 'half a thought, $x^2$';
c.box.oninput();
c.tick();
assert.strictEqual(c.keeps.length, 1, 'one keep for a burst of changes');
const draft = c.keeps[0];
assert.deepStrictEqual(JSON.parse(draft), { pick: 1, sure: 3, work: 'half a thought, $x^2$' });
c = run(question, [], draft);
assert.ok(!c.card.classList.contains('done'));
assert.deepStrictEqual(on(c.options), [false, true, false, false]);
assert.deepStrictEqual(on(c.sure), [false, false, false, true]);
assert.strictEqual(c.box.value, 'half a thought, $x^2$');
assert.ok(!c.main.disabled && !c.box.readOnly, 'still theirs to finish');
// Answered: nothing is kept any longer, and the answer wins over an old draft.
c.main.onclick();
assert.strictEqual(c.keeps[c.keeps.length - 1], '');
c = run(question, [answer], draft);
assert.deepStrictEqual(on(c.options), [false, false, true, false]);
assert.strictEqual(c.box.value, "I honestly\ndon't remember");
// A draft that isn't one (another page's text, a pick out of range) changes nothing.
c = run(question, [], 'not json');
assert.strictEqual(c.box.value, '');
c = run(question, [], JSON.stringify({ pick: 9, sure: 'x', work: 5 }));
assert.deepStrictEqual(on(c.options), [false, false, false, false]);
assert.strictEqual(c.box.value, '');
// A problem's working sent in pieces: the box empties, and so does what's kept.
c = run(problem, [], JSON.stringify({ pick: null, sure: null, work: 'so far' }));
assert.strictEqual(c.box.value, 'so far');
c.card.all((e) => e.tag === 'button' && e.textContent === 'Send working')[0].onclick();
c.tick();
assert.strictEqual(c.keeps[c.keeps.length - 1], '');
process.stdout.write('ok');
