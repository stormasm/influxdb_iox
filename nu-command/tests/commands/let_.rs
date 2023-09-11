use nu_test_support::nu;

#[test]
fn let_name_builtin_var() {
    let actual = nu!("let in = 3");

    assert!(actual
        .err
        .contains("'in' is the name of a builtin Nushell variable"));
}

#[test]
fn let_doesnt_mutate() {
    let actual = nu!("let i = 3; $i = 4");

    assert!(actual.err.contains("immutable"));
}

#[test]
fn let_takes_pipeline() {
    let actual = nu!(r#"let x = "hello world" | str length; print $x"#);

    assert_eq!(actual.out, "11");
}

#[test]
fn let_pipeline_allows_in() {
    let actual =
        nu!(r#"def foo [] { let x = $in | str length; print ($x + 10) }; "hello world" | foo"#);

    assert_eq!(actual.out, "21");
}

#[test]
fn mut_takes_pipeline() {
    let actual = nu!(r#"mut x = "hello world" | str length; print $x"#);

    assert_eq!(actual.out, "11");
}

#[test]
fn mut_pipeline_allows_in() {
    let actual =
        nu!(r#"def foo [] { mut x = $in | str length; print ($x + 10) }; "hello world" | foo"#);

    assert_eq!(actual.out, "21");
}

#[ignore]
#[test]
fn let_with_external_failed() {
    // FIXME: this test hasn't run successfully for a long time. We should
    // bring it back to life at some point.
    let actual = nu!(r#"let x = nu --testbin outcome_err "aa"; echo fail"#);

    assert!(!actual.out.contains("fail"));
}