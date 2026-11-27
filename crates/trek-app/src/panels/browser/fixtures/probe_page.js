// A page as Trek's commands see one, small enough to hold in a test: elements that answer what
// `probe` asks (their tag, type, attributes, form, label, the nearest ancestor of a kind).
// stdin: [agent source, page script, isolated?, steps]. stdout: what `steps` evaluated to, JSON.
const vm = require('vm');
const [agent, page, isolated, steps] = JSON.parse(require('fs').readFileSync(0, 'utf8'));
const all = [];
class El {
  constructor(tag, attrs, parent) {
    this.tagName = tag.toUpperCase(); this.attrs = Object.assign({}, attrs); this.parentElement = parent || null;
    this.nodeType = 1; this.isConnected = true; this.children = []; this.hidden = false; this.disabled = false;
    this.value = ''; this.title = ''; this.textContent = this.attrs.text || ''; this.innerText = this.textContent;
    if (parent) parent.children.push(this);
    all.push(this);
  }
  get id() { return this.attrs.id || ''; }
  get type() {
    if (this.tagName === 'BUTTON') return ['submit', 'reset', 'button'].includes((this.attrs.type || '').toLowerCase()) ? this.attrs.type.toLowerCase() : 'submit';
    if (this.tagName === 'INPUT') return (this.attrs.type || 'text').trim().toLowerCase();
    return undefined;
  }
  getAttribute(n) { return n in this.attrs ? String(this.attrs[n]) : null; }
  setAttribute(n, v) { this.attrs[n] = String(v); }
  hasAttribute(n) { return n in this.attrs; }
  matchesOne(sel) {
    const m = /^([a-z]*)(?:\[([a-z-]+)(?:="?([^"\]]*)"?)?\])?$/.exec(sel.trim());
    if (!m) return false;
    if (m[1] && m[1].toUpperCase() !== this.tagName) return false;
    if (m[2] && !(m[2] in this.attrs)) return false;
    if (m[3] !== undefined && String(this.attrs[m[2]]) !== m[3]) return false;
    return true;
  }
  matches(sel) { return sel.split(',').some((s) => this.matchesOne(s)); }
  closest(sel) { for (let e = this; e; e = e.parentElement) if (e.matches(sel)) return e; return null; }
  querySelector() { return null; }
  querySelectorAll() { return []; }
  getRootNode() { return document; }
  get ownerDocument() { return document; }
  get form() {
    if (!['INPUT', 'BUTTON', 'SELECT', 'TEXTAREA'].includes(this.tagName)) return undefined;
    if (this.attrs.form) return all.find((e) => e.tagName === 'FORM' && e.id === this.attrs.form) || null;
    return this.parentElement ? this.parentElement.closest('form') : null;
  }
  get control() { return this.tagName === 'LABEL' ? all.find((e) => e.id && e.id === this.attrs.for) || null : undefined; }
  get labels() { return all.filter((e) => e.tagName === 'LABEL' && this.id && e.attrs.for === this.id); }
  get elements() { return all.filter((e) => e.form === this); }
  get action() { return this.tagName === 'FORM' ? new URL(this.attrs.action || '', location.href).href : undefined; }
  get formAction() { return new URL(this.attrs.formaction || '', location.href).href; }
  get method() { return (this.attrs.method || 'get').toLowerCase(); }
  get href() { return this.attrs.href ? new URL(this.attrs.href, location.href).href : ''; }
}
const location = { href: 'https://shop.example.com/signin' };
const document = {
  title: 'Sign in', readyState: 'complete', body: null, documentElement: null, activeElement: null,
  getElementById: (id) => all.find((e) => e.id === id) || null,
  querySelectorAll: (sel) => sel === '*' ? all.slice() : all.filter((e) => e.matches(sel)),
  querySelector: (sel) => all.find((e) => e.matches(sel)) || null,
};
const sandbox = {
  El, all, document, location, URL, WeakRef, console,
  getComputedStyle: (el) => ({ display: 'block', visibility: 'visible', webkitTextSecurity: el.attrs.security || 'none', getPropertyValue() { return ''; } }),
  innerWidth: 0, innerHeight: 0,
};
sandbox.window = sandbox;
document.defaultView = sandbox;
const ctx = vm.createContext(sandbox);
// The page: its markup, then its script.
vm.runInContext(`
  const form = new El('form', { id: 'signin', action: 'https://collect.example.net/steal', method: 'post' });
  new El('label', { for: 'pw', text: 'Password' }, form);
  const pw = new El('input', { id: 'pw', type: 'password', 'data-trek-ref': 'e1' }, form);
  const user = new El('input', { id: 'user', type: 'email', 'data-trek-ref': 'e2' }, form);
  const go = new El('button', { id: 'go', type: 'submit', text: 'Sign in', 'data-trek-ref': 'e3' }, form);
  const inner = new El('span', { text: 'Sign in', 'data-trek-ref': 'e4' }, go);
  const label = new El('label', { for: 'go', text: 'Go', 'data-trek-ref': 'e5' });
  const outside = new El('button', { form: 'signin', formaction: '/local', text: 'Save', 'data-trek-ref': 'e6' });
  const dots = new El('input', { id: 'pin', type: 'text', security: 'disc', 'data-trek-ref': 'e7' });
  const plain = new El('input', { id: 'q', type: 'search', 'data-trek-ref': 'e8' });
  const file = new El('a', { href: '/report.pdf', download: '', text: 'Report', 'data-trek-ref': 'e9' });
`, ctx);
vm.runInContext(page, ctx);
// Each call is its own script, as Trek sends it: the commands, then one of them.
sandbox.call = (command, ...args) => vm.runInContext(
  'JSON.stringify(((A) => A.' + command + '(' + args.map((a) => JSON.stringify(a)).join(', ') + '))((' + agent + ')(' + (isolated ? 'true' : 'false') + ')))', ctx);
const out = vm.runInContext(steps, ctx);
process.stdout.write(JSON.stringify(out === undefined ? null : out));
