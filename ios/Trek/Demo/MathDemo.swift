import Foundation

// MARK: Maths and a live page

extension DemoMac {
    /// The question the maths thread (`t-math`) starts with.
    static let mathAsk = "Explain Bayes’ rule with the numbers from the screening example, and give me a little calculator for it."

    /// An answer with maths in every form the Mac typesets — inline `$…$` and `\(…\)`, display
    /// `$$…$$` and `\[…\]`, matrices, cases — prices that must stay prose, and a `trek-html` page.
    static let mathAnswer = #"""
    ## Bayes’ rule

    For a test $T$ and a condition $\theta$, the chance you have it once the test says so is \(P(\theta=1 \mid T=+)\):

    $$
    P(\theta \mid T) = \frac{P(T \mid \theta)\,P(\theta)}{\sum_{\theta'} P(T \mid \theta')\,P(\theta')}
    $$

    With a prevalence of $p = 0.001$, a sensitivity of $0.99$ and a false-positive rate of $\alpha = 0.06$:

    \[ P(\theta=1\mid T=+) = \frac{0.99 \times 0.001}{0.99 \times 0.001 + 0.06 \times 0.999} \approx \boxed{0.0162} \]

    So under **2%**, even after a positive test. The screening costs $5 and the follow-up $10, which is why it's done in that order.

    - Newton's step: $x_{n+1} = x_n - \frac{f(x_n)}{f'(x_n)}$, with $\sqrt{2} \approx 1.4142$
    - Accents: $\hat\theta$, $\bar{x}$, $\vec{v}$, and $\alpha \le \beta \ne \gamma$
    - Euler: $e^{i\pi} + 1 = 0$, and the $n$th term of $\sum_{k=1}^{n} k = \frac{n(n+1)}{2}$

    $$\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}, \qquad \lim_{n\to\infty}\left(1 + \frac{1}{n}\right)^n = e$$

    $$
    A = \begin{pmatrix} 1 & 2 \\ 3 & 4 \end{pmatrix}, \quad |x| = \begin{cases} x & x \ge 0 \\ -x & x < 0 \end{cases}
    $$

    And one this renderer doesn't know: $\weirdmacro{x}$.

    ```trek-html
    <!doctype html>
    <html>
    <head>
    <title>Bayes calculator</title>
    <style>
      body { margin: 0; padding: 16px; font-family: var(--trek-font); color: var(--trek-fg); background: transparent; }
      label { display: flex; justify-content: space-between; align-items: center; gap: 12px; margin: 10px 0; color: var(--trek-muted); font-size: 14px; }
      input[type=range] { flex: 1; accent-color: var(--trek-accent); }
      output { font-family: var(--trek-mono); color: var(--trek-fg); min-width: 52px; text-align: right; }
      .result { margin-top: 14px; padding: 12px 14px; border-radius: var(--trek-radius); background: var(--trek-surface); border: 1px solid var(--trek-border); }
      .big { font-size: 28px; font-weight: 600; color: var(--trek-accent); }
    </style>
    </head>
    <body>
      <label>Prevalence <input id="p" type="range" min="0.0001" max="0.2" step="0.0001" value="0.001"><output id="po"></output></label>
      <label>Sensitivity <input id="s" type="range" min="0.5" max="1" step="0.001" value="0.99"><output id="so"></output></label>
      <label>False positives <input id="f" type="range" min="0" max="0.2" step="0.001" value="0.06"><output id="fo"></output></label>
      <div class="result">Chance of having it after a positive test<div class="big" id="r"></div></div>
      <script>
        const $ = id => document.getElementById(id);
        function update() {
          const p = +$('p').value, s = +$('s').value, f = +$('f').value;
          $('po').textContent = (p * 100).toFixed(2) + '%';
          $('so').textContent = (s * 100).toFixed(1) + '%';
          $('fo').textContent = (f * 100).toFixed(1) + '%';
          const post = s * p / (s * p + f * (1 - p));
          $('r').textContent = (post * 100).toFixed(2) + '%';
        }
        for (const id of ['p', 's', 'f']) $(id).addEventListener('input', update);
        update();
      </script>
    </body>
    </html>
    ```

    Drag the sliders to see how much the prevalence matters.
    """#
}
