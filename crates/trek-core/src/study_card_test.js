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

function run(data, replies) {
  const card = new El('div');
  const listeners = {}, sent = [], dictated = [];
  const window = {
    trek: { send: (t) => (sent.push(t), true), copy: () => true, replies, canDictate: true, dictate: (on) => (dictated.push(on), true) },
    addEventListener: (t, f) => { (listeners[t] = listeners[t] || []).push(f); },
  };
  const document = { createElement: (t) => new El(t), getElementById: () => card, addEventListener: () => {} };
  vm.runInContext(script.replace('__DATA__', JSON.stringify(data)), vm.createContext({ window, document, navigator: { platform: 'MacIntel' } }));
  const cls = (c) => (e) => e.classList.contains(c);
  return {
    card, sent, dictated,
    hear: (detail) => (listeners.trekdictation || []).forEach((f) => f({ detail })),
    box: card.all((e) => e.tag === 'textarea')[0],
    mic: card.all(cls('mic'))[0],
    options: card.all(cls('opt')),
    sure: card.all((e) => e.parent && e.parent.classList.contains('sure')),
    main: card.all(cls('go'))[0],
    line: () => (card.all(cls('sent'))[0] || { text: () => null }).text(),
    note: () => card.all(cls('note'))[0].textContent,
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
assert.strictEqual(c.line(), 'Sent · C. Population growth $\\bar n$ · Unsure (45%)');
// "I don't know": closed, nothing chosen.
c = run(question, ["Question\nAnswer: I don't know\nWorking: (none)"]);
assert.ok(c.card.classList.contains('done'));
assert.deepStrictEqual(on(c.options), [false, false, false, false]);
assert.strictEqual(c.box.value, '');
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
assert.ok(c.card.classList.contains('done') && c.box.readOnly);
assert.deepStrictEqual(on(c.options), [false, false, true, false]);
assert.deepStrictEqual(on(c.sure), [false, true, false, false]);
assert.strictEqual(c.box.value, 'I think it is population growth');
// Heard after it closed: nothing changes.
c.hear({ state: 'done', text: 'late' });
assert.strictEqual(c.box.value, 'I think it is population growth');

// A problem stays open for more working until it's solved.
const problem = { kind: 'problem', step: '', prompt: 'x', options: [], targets: ['c2'] };
c = run(problem, ['Problem [c2]\nStatus: still working\nWorking: abc']);
assert.ok(!c.card.classList.contains('done'));
assert.strictEqual(c.note(), 'Sent · keep going');
c = run(problem, ['Problem [c2]\nStatus: still working\nWorking: abc', "Problem [c2]\nStatus: I think I've solved it\nWorking: got it"]);
assert.ok(c.card.classList.contains('done'));
assert.strictEqual(c.box.value, 'got it');
process.stdout.write('ok');
