use super::*;

#[test]
fn bundled_drivers_have_url_templates() {
    assert_eq!(
        url_template("postgresql", None).as_deref(),
        Some("jdbc:postgresql://localhost:5432/postgres")
    );
    assert_eq!(
        url_template("mysql", None).as_deref(),
        Some("jdbc:mysql://localhost:3306/")
    );
    assert_eq!(
        url_template("mariadb", None).as_deref(),
        Some("jdbc:mariadb://localhost:3306/")
    );
    assert_eq!(url_template("oracle", None), None, "user drivers have none");
}

#[test]
fn sqlite_template_points_into_the_project_root() {
    assert_eq!(
        url_template("sqlite", Some("/home/me/petshop")).as_deref(),
        Some("jdbc:sqlite:/home/me/petshop/db.sqlite")
    );
    assert_eq!(
        url_template("sqlite", Some("C:\\work\\petshop\\")).as_deref(),
        Some("jdbc:sqlite:C:/work/petshop/db.sqlite"),
        "forward slashes, no doubled separator"
    );
    assert_eq!(
        url_template("sqlite", None).as_deref(),
        Some("jdbc:sqlite:./db.sqlite")
    );
}

#[test]
fn switching_drivers_replaces_only_untouched_urls() {
    let root = Some("/p");
    assert_eq!(
        url_after_driver_change(
            "jdbc:postgresql://localhost:5432/postgres",
            "postgresql",
            "mysql",
            root
        ),
        "jdbc:mysql://localhost:3306/"
    );
    assert_eq!(
        url_after_driver_change("", "oracle", "sqlite", root),
        "jdbc:sqlite:/p/db.sqlite"
    );
    assert_eq!(
        url_after_driver_change(
            "jdbc:postgresql://db.internal/app",
            "postgresql",
            "mysql",
            root
        ),
        "jdbc:postgresql://db.internal/app",
        "a typed URL is kept"
    );
    assert_eq!(
        url_after_driver_change("jdbc:mysql://localhost:3306/", "mysql", "oracle", root),
        "",
        "a driver without a template clears the previous template"
    );
}

#[test]
fn password_is_kept_replaced_or_forgotten() {
    assert_eq!(password_to_send("", false), None);
    assert_eq!(
        password_to_send(" secret ", false).as_deref(),
        Some(" secret ")
    );
    assert_eq!(password_to_send("", true).as_deref(), Some(""));
    assert_eq!(
        password_to_send("new", true).as_deref(),
        Some("new"),
        "a typed password wins over Forget"
    );
}

#[test]
fn jar_paths_split_on_commas_and_newlines() {
    assert_eq!(
        parse_jar_paths(" /a/x.jar, /b/y.jar\n\n/c/z.jar ,"),
        ["/a/x.jar", "/b/y.jar", "/c/z.jar"]
    );
    assert!(parse_jar_paths(" , \n").is_empty());
}

#[test]
fn driver_ids_are_slugs_of_the_name() {
    assert_eq!(driver_id_for_name("Oracle"), "oracle");
    assert_eq!(
        driver_id_for_name("  MS SQL Server 2022! "),
        "ms-sql-server-2022"
    );
    assert_eq!(driver_id_for_name("--"), "");
}

#[test]
fn test_results_mention_the_server_version() {
    let result = TestConnectionResult {
        ok: true,
        message: "Connected".to_string(),
        server_version: Some("PostgreSQL 16.2".to_string()),
    };
    assert_eq!(test_result_text(&result), "Connected (PostgreSQL 16.2)");
    let failed = TestConnectionResult {
        ok: false,
        message: "password authentication failed".to_string(),
        server_version: None,
    };
    assert_eq!(test_result_text(&failed), "password authentication failed");
}
