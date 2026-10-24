mod single {
    jt::load!("tests/views/user.jt");
}

mod tree {
    jt::load_dir!("tests/views");
}

#[test]
fn load_one_file() {
    let account = single::Account {
        id: 1,
        name: "Ada Lovelace",
        email: None,
        roles: vec!["admin", "dev"],
    };
    assert_eq!(
        jt::to_string(&single::User(&account)),
        r#"{"id":1,"name":"Ada Lovelace","initials":"AL","roles":["admin","dev"],"admin":true}"#
    );
}

#[test]
fn load_a_directory_as_modules() {
    let page = tree::notion::create_page::CreatePage("db1", "Hello");
    assert_eq!(
        jt::to_string(&page),
        r#"{"parent":{"database_id":"db1"},"properties":{"Name":{"title":[{"text":{"content":"Hello"}}]}}}"#
    );
    let account = tree::user::Account {
        id: 2,
        name: "x",
        email: Some("x@y"),
        roles: vec![],
    };
    assert!(jt::to_string(&tree::user::User(&account)).contains(r#""email":"x@y""#));
}

mod bracketed {
    // `[id].jt` is a template for the route `/users/{id}`, which is the file
    // name the whole convention is built on. The module name has to be the
    // one rsx and brocanth-pages give it, or a route and its template land
    // on different paths and neither finds the other.
    jt::load_dir!("tests/views");

    #[test]
    fn a_bracketed_name_reaches_the_module_rsx_would_give_it() {
        let out = jt::to_string(&users::_id_::Detail(7, "ada"));
        assert_eq!(out, r#"{"id":7,"name":"ada"}"#);
    }
}
