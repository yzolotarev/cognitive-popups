use super::*;

#[test]
fn text_is_split_at_both_kinds_of_delimiters() {
    let s = segments(r"Шаг: \(x_1 = 1,5\), затем \[ x_2 \approx 1{,}4167 \] и всё. $B$3 остаётся.");
    assert_eq!(s.len(), 5);
    assert_eq!(
        s[1],
        Segment::Math {
            tex: "x_1 = 1,5".into(),
            display: false
        }
    );
    assert!(matches!(&s[3], Segment::Math { display: true, .. }));
    assert_eq!(s[4], Segment::Text(" и всё. $B$3 остаётся.".into()));
}

#[test]
fn an_unclosed_formula_is_text_without_markup() {
    let s = segments(r"начало \(\frac{a}{b} без конца");
    assert_eq!(s.len(), 2);
    let Segment::Text(text) = &s[1] else { panic!() };
    assert!(!text.contains('\\') && !text.contains('{'), "{text}");
}

#[test]
fn the_whole_corpus_parses() {
    for tex in CORPUS {
        assert!(parse(tex).is_ok(), "{tex}: {:?}", parse(tex));
    }
}

#[test]
fn unknown_commands_are_refused_not_guessed() {
    assert_eq!(parse(r"\foo{x}"), Err(FormulaError::Unknown("foo".into())));
    assert!(parse(r"\frac{1}{").is_err());
    assert!(parse(r"x^").is_err());
    assert!(parse(r"\left( x").is_err());
    assert!(parse(r"\begin{align} x \end{align}").is_err());
}

#[test]
fn unicode_reads_like_handwriting() {
    let u = |tex| to_unicode(&parse(tex).unwrap());
    assert_eq!(
        u(r"x_{n+1} = x_n - \frac{f(x_n)}{f'(x_n)}"),
        "xₙ₊₁ = xₙ − (f(xₙ))/(f′(xₙ))"
    );
    assert_eq!(u(r"\sqrt{2} \approx 1.414"), "√2 ≈ 1.414");
    assert_eq!(u(r"x \in \mathbb{R}^n"), "x ∈ ℝⁿ");
    assert_eq!(u(r"\frac{1}{2x}"), "1/(2x)");
    assert_eq!(u(r"\lim_{x \to 0} \frac{\sin x}{x}"), "lim_(x→0) (sin x)/x");
    assert_eq!(u(r"1,5 + 0{,}5"), "1,5 + 0,5");
    assert_eq!(u(r"e^{i\pi}"), "e^(iπ)");
}

#[test]
fn typst_escapes_what_would_change_meaning() {
    let t = |tex| to_typst(&parse(tex).unwrap());
    // A literal slash is not a Typst fraction; commas do not split arguments.
    assert_eq!(t("a/b"), r"a \/ b");
    assert_eq!(t(r"\frac{f(x,y)}{3}"), r"frac(f \( x \, y \), 3)");
    assert_eq!(t(r"\sqrt[3]{27}"), "root(3, 27)");
    assert_eq!(t(r"x_{n+1}"), "x_(n + 1)");
    assert_eq!(t(r"\operatorname{rank} A"), "op(\"rank\") A");
    assert_eq!(t(r"1,5"), r#"1 class("normal", \,) 5"#);
    assert_eq!(t(r"1{,}5"), t(r"1,5"));
    // Letters stay separate: `ac` would be one Typst variable.
    assert_eq!(t(r"4ac"), "4 a c");
}

#[test]
fn plain_text_never_shows_markup() {
    let text = r"Ответ: \(\frac{1}{3}\); а \(\unknown{x}^2\) и \[\sqrt{2}\].";
    let plain = plain(text);
    assert_eq!(plain, "Ответ: 1/3; а x^2 и √2.");
    for tex in CORPUS {
        let u = unicode_of(tex);
        assert!(!u.contains('\\'), "{tex} → {u}");
    }
    assert!(!loose(r"\frac{a}{b} + \alpha_{1}").contains('\\'));
}

#[test]
fn a_broken_fraction_still_reads_as_a_fraction() {
    assert_eq!(loose(r"\frac{a+1}{b} \foo"), "(a+1)/(b)");
}

#[test]
fn plain_four_words_cue_with_fraction_has_no_latex() {
    // A cue meaning as the model wrote it: the window shows `plain` of it.
    let text = r"Формула корней \(x_{1,2} = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}\)";
    let shown = plain(text);
    assert!(!shown.contains('\\') && !shown.contains("frac"), "{shown}");
}

#[test]
fn plain_lesson_step_label_has_no_latex_delimiters() {
    // A plan item as the model wrote it; the lesson window shows `plain` of it.
    let text = r"Составить выражение для разности \(f(x + h) - f(x)\) для заданной функции.";
    let shown = plain(text);
    assert!(!shown.contains('\\'), "{shown}");
    assert!(shown.contains("f(x + h) − f(x)"), "{shown}");
}

#[test]
fn the_formulas_the_study_text_prompt_asks_for_parse_and_read_as_unicode() {
    // B85: the examples and the command set written into NEW_study_text.md.
    for tex in [
        r"C_n^k = \dfrac{n!}{k!\,(n-k)!}",
        r"E(L) = 0{,}3 \cdot 25 + 0{,}7 \cdot 0 = 7{,}5",
        r"P(A \mid B) = \frac{P(A \cap B)}{P(B)}",
        r"\sigma = \sqrt{\sum_{i=1}^{n} (x_i - \mu)^2}",
        r"\mathbb{P}(X \le 3) \approx \overline{x} \ne \text{среднее}",
        r"\left( \int_0^1 x^2 \, dx \right)",
    ] {
        assert!(parse(tex).is_ok(), "does not parse: {tex}");
        assert!(!unicode_of(tex).contains('\\'), "raw LaTeX left: {tex}");
    }
    // \binom is outside the subset: the prompt tells the model not to use it.
    assert!(parse(r"\binom{n}{k}").is_err());
}
