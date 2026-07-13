#[test]
fn every_audited_fix_has_implementation_and_validation_evidence() {
    let status = std::process::Command::new("python3")
        .arg("scripts/check_fixes_validation.py")
        .status()
        .expect("FIXES.md evidence checker should start");

    assert!(status.success(), "FIXES.md evidence checker failed");
}
