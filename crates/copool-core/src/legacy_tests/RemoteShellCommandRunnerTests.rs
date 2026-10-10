use crate::remote::*;
#[test]
fn testSSHArgumentsIncludeConfiguredPort() {
    let mut server = draft("server-1");
    server.ssh_port = 2222;
    server.host = "example.com".into();
    server.identity_file = Some("/tmp/id_ed25519".into());
    let args = ssh_arguments(&server, "echo ok");
    assert_eq!(&args[..3], ["ssh", "-p", "2222"]);
    assert!(args.contains(&"root@example.com".into()));
    assert!(args.contains(&"echo ok".into()));
}
#[test]
fn testWithRootPrivilegesTrimsTrailingSemicolonsFromInlineCommand() {
    let command = with_root("echo ok;");
    assert!(!command.contains(";; else"));
    assert_eq!(
        command,
        "if [ \"$(id -u)\" = \"0\" ]; then echo ok; else sudo sh -lc 'echo ok'; fi"
    );
}
