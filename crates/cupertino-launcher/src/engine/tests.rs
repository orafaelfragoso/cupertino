use super::*;
use crate::desktop::{DesktopAction, DesktopEntry};
use crate::files::FileSearchConfig;

pub fn app(id: &str, name: &str, generic: &str, exec: &str) -> DesktopEntry {
    DesktopEntry {
        id: format!("{id}.desktop"),
        name: name.into(),
        generic_name: generic.into(),
        comment: String::new(),
        keywords: vec![],
        categories: vec![],
        icon: Some(id.into()),
        exec: exec.into(),
        try_exec: None,
        terminal: false,
        path: None,
        startup_wm_class: None,
        actions: vec![],
        source: None,
    }
}

fn entries() -> Vec<DesktopEntry> {
    let mut browser = app("browser", "Firefox", "Web Browser", "firefox %u");
    browser.keywords = vec!["internet".into(), "www".into()];
    browser.categories = vec!["Network".into(), "WebBrowser".into()];
    browser.actions = vec![
        DesktopAction {
            id: "new-window".into(),
            name: "New Window".into(),
            exec: "firefox --new-window".into(),
            icon: None,
        },
        DesktopAction {
            id: "private".into(),
            name: "New Private Window".into(),
            exec: "firefox --private-window".into(),
            icon: None,
        },
    ];
    let mut htop = app("htop", "Htop", "Process Viewer", "htop");
    htop.terminal = true;
    let mut calc = app(
        "org.gnome.Calculator",
        "Calculator",
        "Calculator",
        "gnome-calculator",
    );
    calc.comment = "Perform arithmetic and scientific calculations".into();
    calc.categories = vec!["Utility".into()];
    let mut gimp = app("gimp", "GIMP", "Image Editor", "gimp-2.10 %U");
    gimp.categories = vec!["Graphics".into()];
    let mut code = app("code", "Visual Studio Code", "Text Editor", "code %F");
    code.categories = vec!["Development".into(), "IDE".into()];
    vec![
        browser,
        code,
        app(
            "writer",
            "LibreOffice Writer",
            "Word Processor",
            "libreoffice --writer %U",
        ),
        app(
            "terminal",
            "Terminal",
            "Terminal Emulator",
            "gnome-terminal",
        ),
        app("files", "Files", "File Manager", "nautilus %U"),
        htop,
        calc,
        gimp,
        app("firewall", "Firewall Configuration", "", "firewall-config"),
    ]
}

pub fn test_launcher() -> Launcher {
    Launcher::new(LauncherConfig::default(), entries(), Frecency::in_memory())
}

fn launcher() -> Launcher {
    test_launcher()
}

fn titles(r: &Results) -> Vec<&str> {
    r.iter().map(|i| i.title.as_str()).collect()
}

const NOW: f64 = 1_000_000.0;

#[test]
fn prefix_query_ranks_name_prefix_first() {
    let r = launcher().query("fire", NOW);
    let t = titles(&r);
    assert_eq!(t[0], "Firefox", "{t:?}");
    assert!(t.contains(&"Firewall Configuration"));
    assert_eq!(
        r.sections[0].kind,
        SectionKind::TopHit,
        ": the best match leads as the single top hit"
    );
    assert_eq!(r.sections[0].items.len(), 1);
    assert_eq!(r.sections[1].kind, SectionKind::Applications);
}

#[test]
fn acronyms_generic_names_keywords_and_exec_names_match() {
    let l = launcher();
    assert_eq!(titles(&l.query("vsc", NOW))[0], "Visual Studio Code");
    assert_eq!(titles(&l.query("libre wr", NOW))[0], "LibreOffice Writer");
    assert_eq!(titles(&l.query("web browser", NOW))[0], "Firefox");
    assert_eq!(titles(&l.query("internet", NOW))[0], "Firefox");
    assert_eq!(titles(&l.query("nautilus", NOW))[0], "Files");
    assert_eq!(
        titles(&l.query("scientific", NOW))[0],
        "Calculator",
        "comment words match"
    );
    assert_eq!(titles(&l.query("gnome-calc", NOW))[0], "Calculator");
}

#[test]
fn desktop_actions_only_appear_for_action_words() {
    let l = launcher();
    let t = titles(&l.query("fire", NOW))
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
    assert!(!t.iter().any(|x| x.contains("New Window")), "{t:?}");
    let r = l.query("private", NOW);
    assert_eq!(r.get(0).unwrap().title, "Firefox: New Private Window");
    assert_eq!(r.get(0).unwrap().kind, ItemKind::ApplicationAction);
    let r = l.query("firefox private", NOW);
    assert_eq!(r.get(0).unwrap().title, "Firefox: New Private Window");
    let off = LauncherConfig {
        include_actions: false,
        ..LauncherConfig::default()
    };
    let l = Launcher::new(off, entries(), Frecency::in_memory());
    assert!(l.query("private", NOW).is_empty());
}

#[test]
fn calculator_comes_first_and_copies() {
    let mut l = launcher();
    let r = l.query("12*(3+4)", NOW);
    assert_eq!(r.sections[0].kind, SectionKind::Calculator);
    let item = r.get(0).unwrap().clone();
    assert_eq!(item.title, "84");
    assert_eq!(item.subtitle, "12*(3+4)");
    assert_eq!(
        l.activate(&item, "12*(3+4)", NOW),
        Activation::CopyToClipboard("84".into())
    );
    assert!(l.frecency.is_empty(), "copying is not recorded");
    assert!(l
        .query("firefox", NOW)
        .sections
        .iter()
        .all(|s| s.kind != SectionKind::Calculator));
    let cfg = LauncherConfig {
        show_calculator: false,
        ..LauncherConfig::default()
    };
    assert!(Launcher::new(cfg, entries(), Frecency::in_memory())
        .query("1+1", NOW)
        .sections
        .iter()
        .all(|s| s.kind != SectionKind::Calculator));
}

#[test]
fn frecency_breaks_ties_between_equal_text_matches() {
    let mut l = Launcher::new(
        LauncherConfig::default(),
        vec![
            app("a", "Editor One", "", "a"),
            app("b", "Editor Two", "", "b"),
        ],
        Frecency::in_memory(),
    );
    let first = titles(&l.query("edit", NOW))[0].to_string();
    assert_eq!(first, "Editor One", "alphabetical without history");
    let two = l
        .query("edit", NOW)
        .iter()
        .find(|i| i.title == "Editor Two")
        .unwrap()
        .clone();
    for _ in 0..3 {
        l.activate(&two, "edit", NOW);
    }
    assert_eq!(titles(&l.query("edit", NOW + 10.0))[0], "Editor Two");
    assert_eq!(titles(&l.query("ed", NOW + 10.0))[0], "Editor Two");
}

#[test]
fn frecency_does_not_outrank_a_better_text_tier() {
    let mut l = launcher();
    let writer = l.query("libreoffice writer", NOW).get(0).unwrap().clone();
    for _ in 0..200 {
        l.activate(&writer, "", NOW);
    }
    assert_eq!(titles(&l.query("term", NOW))[0], "Terminal");
}

#[test]
fn an_empty_query_lists_nothing_and_the_applications_token_alone_lists_the_frecent_ones() {
    let mut l = launcher();
    assert!(l.query("", NOW).is_empty());
    assert!(l.query("/apps", NOW).is_empty());
    let htop = l.query("htop", NOW).get(0).unwrap().clone();
    let gimp = l.query("gimp", NOW).get(0).unwrap().clone();
    l.activate(&htop, "htop", NOW);
    l.activate(&gimp, "gimp", NOW);
    l.activate(&gimp, "gimp", NOW + 5.0);
    assert!(
        l.query("", NOW + 10.0).is_empty(),
        "the bare field shows no results"
    );
    let r = l.query("/apps ", NOW + 10.0);
    assert_eq!(r.sections[0].kind, SectionKind::Suggestions);
    assert_eq!(titles(&r), vec!["GIMP", "Htop"]);
}

#[test]
fn launch_builds_command_and_records_frecency() {
    let mut l = launcher();
    let item = l.query("visual studio", NOW).get(0).unwrap().clone();
    let Some(cmd) = l.activate(&item, "visual studio", NOW).into_command() else {
        panic!("expected a command")
    };
    assert_eq!(cmd.argv(), ["code"], "file field codes are stripped");
    assert_eq!(cmd.desktop_id.as_deref(), Some("code.desktop"));
    assert_eq!(l.frecency.launches("app:code.desktop"), 1);
    assert!(
        l.frecency.bonus("app:code.desktop", "visual st", NOW)
            > l.frecency.bonus("app:code.desktop", "", NOW),
        "query association learned"
    );
}

#[test]
fn action_launch_and_terminal_wrapping() {
    let mut l = launcher();
    let item = l.query("private", NOW).get(0).unwrap().clone();
    let Some(cmd) = l.activate(&item, "private", NOW).into_command() else {
        panic!()
    };
    assert_eq!(cmd.argv(), ["firefox", "--private-window"]);
    let htop = l.query("htop", NOW).get(0).unwrap().clone();
    let Some(cmd) = l.activate(&htop, "htop", NOW).into_command() else {
        panic!()
    };
    assert_eq!(cmd.argv(), ["xterm", "-e", "htop"]);
    l.config.terminal_command = vec!["foot".into()];
    let Some(cmd) = l.activate(&htop, "htop", NOW).into_command() else {
        panic!()
    };
    assert_eq!(cmd.argv(), ["foot", "htop"]);
}

#[test]
fn an_application_opens_as_its_dock_tile_and_a_desktop_action_runs() {
    let mut l = launcher();
    let firefox = l.query("firefox", NOW).get(0).unwrap().clone();
    let Activation::OpenApp { desktop_id, spec } = l.activate(&firefox, "firefox", NOW) else {
        panic!("the application itself is opened")
    };
    assert_eq!(desktop_id, "browser.desktop");
    assert_eq!(spec.argv(), ["firefox"]);
    let private = l.query("private", NOW).get(0).unwrap().clone();
    assert!(
        matches!(l.activate(&private, "private", NOW), Activation::Run(_)),
        "a desktop action is a command of its own"
    );
}

#[test]
fn activation_token_hook_sets_environment() {
    let mut l = launcher();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let s2 = seen.clone();
    l.set_activation_token_hook(Some(Box::new(move |req| {
        s2.lock().unwrap().push(req.clone());
        Some("tok-123".into())
    })));
    let item = l.query("gimp", NOW).get(0).unwrap().clone();
    let Some(cmd) = l.activate(&item, "gimp", NOW).into_command() else {
        panic!()
    };
    assert!(cmd
        .env
        .contains(&("XDG_ACTIVATION_TOKEN".to_string(), "tok-123".to_string())));
    assert!(cmd
        .env
        .contains(&("DESKTOP_STARTUP_ID".to_string(), "tok-123".to_string())));
    let req = seen.lock().unwrap()[0].clone();
    assert_eq!(req.desktop_id.as_deref(), Some("gimp.desktop"));
    assert_eq!(req.title, "GIMP");
    l.set_activation_token_hook(Some(Box::new(|_| None)));
    let Some(cmd) = l.activate(&item, "gimp", NOW).into_command() else {
        panic!()
    };
    assert!(cmd.env.is_empty());
}

#[test]
fn unusable_exec_activates_nothing() {
    let mut l = Launcher::new(
        LauncherConfig::default(),
        vec![app("broken", "Broken", "", "\"unterminated")],
        Frecency::in_memory(),
    );
    let item = l.query("broken", NOW).get(0).unwrap().clone();
    assert_eq!(l.activate(&item, "broken", NOW), Activation::Nothing);
    assert!(l.frecency.is_empty());
}

#[test]
fn a_kind_token_keeps_the_results_of_its_kind() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("Calculations")).unwrap();
    std::fs::write(d.path().join("calculator-notes.txt"), "x").unwrap();
    let mut l = launcher();
    l.set_file_index(Some(Arc::new(FileIndex::build(FileSearchConfig::new(
        d.path(),
    )))));
    let kinds = |r: &Results| -> Vec<SectionKind> { r.sections.iter().map(|s| s.kind).collect() };
    let all = l.query("calc", NOW);
    assert!(all.iter().any(|i| i.kind == ItemKind::Application));
    assert!(all.iter().any(|i| i.kind == ItemKind::File));
    // only the files, wherever the token stands, the top hit taken from them
    for query in ["/files calc", "calc /files"] {
        let r = l.query(query, NOW);
        assert!(!r.is_empty(), "{query}");
        assert!(
            r.iter().all(|i| i.kind == ItemKind::File),
            "{query}: {:?}",
            kinds(&r)
        );
    }
    let r = l.query("/apps calc", NOW);
    assert!(
        r.iter().all(|i| i.kind == ItemKind::Application),
        "{:?}",
        kinds(&r)
    );
    assert!(!kinds(&r).contains(&SectionKind::Web));
    let r = l.query("/folders calc", NOW);
    assert!(r.iter().all(|i| i.kind == ItemKind::Folder) && !r.is_empty());
}

#[test]
fn file_results_form_folder_and_file_sections() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("Projects/reports")).unwrap();
    std::fs::write(d.path().join("Projects/reports/report-2024.pdf"), "x").unwrap();
    std::fs::write(d.path().join("Projects/report.txt"), "x").unwrap();
    let mut l = launcher();
    l.config.top_hit = false;
    l.set_file_index(Some(Arc::new(FileIndex::build(FileSearchConfig::new(
        d.path(),
    )))));
    let r = l.query("report", NOW);
    let kinds: Vec<SectionKind> = r.sections.iter().map(|s| s.kind).collect();
    assert!(
        kinds.contains(&SectionKind::Folders) && kinds.contains(&SectionKind::Files),
        "{kinds:?}"
    );
    let files = r
        .sections
        .iter()
        .find(|s| s.kind == SectionKind::Files)
        .unwrap();
    assert_eq!(files.items.len(), 2);
    assert!(files
        .items
        .iter()
        .all(|i| matches!(i.action, Action::OpenPath(_)) && i.id.starts_with("file:")));
    let item = files.items[0].clone();
    let Some(cmd) = l.activate(&item, "report", NOW).into_command() else {
        panic!()
    };
    assert_eq!(cmd.program, "xdg-open");
    assert_eq!(cmd.args.len(), 1);
    assert!(cmd.args[0].contains("report"));
    assert_eq!(l.frecency.launches(&item.id), 1);
}

#[test]
fn applications_outrank_files_for_application_names() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("firefox-notes.txt"), "x").unwrap();
    let mut l = launcher();
    l.set_file_index(Some(Arc::new(FileIndex::build(FileSearchConfig::new(
        d.path(),
    )))));
    let r = l.query("firefox", NOW);
    assert_eq!(r.sections[0].kind, SectionKind::TopHit);
    assert!(
        matches!(r.sections[0].items[0].action, Action::LaunchApp { .. }),
        "the application is the top hit"
    );
    assert!(r.sections.iter().any(|s| s.kind == SectionKind::Files));
    l.config.top_hit = false;
    let r = l.query("firefox", NOW);
    assert_eq!(r.sections[0].kind, SectionKind::Applications);
}

#[test]
fn web_fallback_is_off_by_default_and_encodes_the_query() {
    assert!(launcher().query("zzzz", NOW).is_empty());
    let cfg = LauncherConfig {
        web_search_url: Some("https://search.example/?q={}".into()),
        ..LauncherConfig::default()
    };
    let mut l = Launcher::new(cfg, entries(), Frecency::in_memory());
    let r = l.query("a&b c", NOW);
    let last = r.sections.last().unwrap();
    assert_eq!(last.kind, SectionKind::Web);
    assert_eq!(
        last.items[0].action,
        Action::OpenUrl("https://search.example/?q=a%26b+c".into())
    );
    let item = last.items[0].clone();
    let Some(cmd) = l.activate(&item, "a&b c", NOW).into_command() else {
        panic!()
    };
    assert_eq!(
        cmd.argv(),
        ["xdg-open", "https://search.example/?q=a%26b+c"]
    );
}

#[test]
fn highlights_index_the_title() {
    let r = launcher().query("vsc", NOW);
    let item = r.get(0).unwrap();
    let chars: Vec<char> = item.title.chars().collect();
    let picked: String = item.highlights.iter().map(|&i| chars[i]).collect();
    assert_eq!(picked, "VSC");
}

#[test]
fn limits_apply_per_section() {
    let many: Vec<DesktopEntry> = (0..30)
        .map(|i| app(&format!("t{i}"), &format!("Tool {i:02}"), "", "t"))
        .collect();
    let l = Launcher::new(
        LauncherConfig {
            top_hit: false,
            ..LauncherConfig::default()
        },
        many.clone(),
        Frecency::in_memory(),
    );
    assert_eq!(l.query("tool", NOW).sections[0].items.len(), 8);
    let l = Launcher::new(LauncherConfig::default(), many, Frecency::in_memory());
    let r = l.query("tool", NOW);
    assert_eq!(
        (r.sections[0].items.len(), r.sections[1].items.len()),
        (1, 7),
        "one top hit, the rest of the 8 applications"
    );
}

#[test]
fn results_helpers() {
    let r = launcher().query("1+1", NOW);
    assert_eq!(r.len(), r.iter().count());
    assert_eq!(r.locate(0), Some((0, 0)));
    assert_eq!(r.locate(r.len()), None);
    assert!(r.get(r.len()).is_none());
    let two = launcher().query("e", NOW);
    assert!(!two.sections.is_empty() && !two.is_empty());
    if two.sections.len() == 1 {
        assert_eq!(two.locate(1), Some((0, 1)));
    }
}

#[test]
fn set_entries_replaces_the_index() {
    let mut l = launcher();
    l.set_entries(vec![app("only", "Only App", "", "only")]);
    assert_eq!(titles(&l.query("app", NOW)), vec!["Only App"]);
    assert!(l.entry("only.desktop").is_some() && l.entry("code.desktop").is_none());
}

#[test]
fn top_hit_is_a_single_item_and_not_duplicated() {
    // MaxCountTopHits = 1.
    let r = launcher().query("fire", NOW);
    let top = &r.sections[0];
    assert_eq!(
        (top.kind, top.items.len(), top.title.as_str()),
        (SectionKind::TopHit, 1, "Top Hit")
    );
    let id = &top.items[0].id;
    assert_eq!(r.iter().filter(|i| &i.id == id).count(), 1);
    // The calculator leads and no extra top hit is created.
    let r = launcher().query("2+2", NOW);
    assert_eq!(r.sections[0].kind, SectionKind::Calculator);
    assert!(r.sections.iter().all(|s| s.kind != SectionKind::TopHit));
}

#[test]
fn default_category_order_lists_documents_before_folders() {
    let pos = |n: &str| DEFAULT_CATEGORY_ORDER.iter().position(|x| *x == n).unwrap();
    assert_eq!(DEFAULT_CATEGORY_ORDER[0], "applications");
    assert!(
        pos("documents") < pos("folders")
            && pos("calculator") < pos("definition")
            && pos("conversion") < pos("calculator")
    );
    assert!((SectionKind::Files as u8) < (SectionKind::Folders as u8));
    assert_eq!(
        (
            reference_limits::VISIBLE_PER_SECTION,
            reference_limits::MAX_TOP_HITS
        ),
        (3, 1)
    );
}

#[test]
fn a_file_offers_open_reveal_and_copy_path_and_reveal_opens_its_folder_in_files() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("budget.ods"), "x").unwrap();
    let mut l = launcher();
    l.set_file_index(Some(Arc::new(FileIndex::build(FileSearchConfig::new(
        d.path(),
    )))));
    let item = l
        .query("budget", NOW)
        .iter()
        .find(|i| i.kind == ItemKind::File)
        .cloned()
        .expect("the file is found");
    let actions = l.actions(&item);
    let labels: Vec<&str> = actions.iter().map(|a| a.label.as_str()).collect();
    assert_eq!(labels, ["Open", "Reveal in Files", "Copy Path"]);
    let folder = d.path().to_string_lossy().into_owned();
    // without the session's file manager the folder opens with the open command
    let Some(open) = l
        .activate_action(&item, &actions[1], "budget", NOW)
        .into_command()
    else {
        panic!()
    };
    assert_eq!(
        (open.program.as_str(), open.args.clone()),
        ("xdg-open", vec![folder.clone()])
    );
    assert_eq!(l.frecency.launches(&item.id), 1);
    // with it, the file manager's entry opens the folder
    let mut entries = l.entries().to_vec();
    entries.push(app(
        "cupertino-files",
        "Files",
        "File Manager",
        "cupertino-files %F",
    ));
    l.set_entries(entries);
    let Some(files) = l
        .activate_action(&item, &actions[1], "budget", NOW)
        .into_command()
    else {
        panic!()
    };
    assert_eq!(files.program, "cupertino-files");
    assert_eq!(files.args, vec![folder]);
    assert_eq!(files.desktop_id.as_deref(), Some(FILE_MANAGER_ID));
    let path = d.path().join("budget.ods").to_string_lossy().into_owned();
    assert_eq!(
        l.activate_action(&item, &actions[2], "budget", NOW),
        Activation::CopyToClipboard(path)
    );
}

#[test]
fn a_folder_reveals_itself_and_values_offer_copy() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(Launcher::reveal_folder(d.path()), d.path());
    assert_eq!(
        Launcher::reveal_folder(&d.path().join("a.txt")),
        d.path().to_path_buf()
    );
    let l = launcher();
    let calc = l.query("2+2", NOW).iter().next().cloned().unwrap();
    assert_eq!(calc.kind, ItemKind::Calculator);
    let labels: Vec<String> = l.actions(&calc).into_iter().map(|a| a.label).collect();
    assert_eq!(labels, ["Copy"]);
}

#[test]
fn a_conversion_leads_the_results_and_copies_its_value() {
    let mut l = launcher();
    let r = l.query("10 km in mi", NOW);
    assert_eq!(r.sections[0].kind, SectionKind::Conversion);
    let item = r.sections[0].items[0].clone();
    assert_eq!(item.kind, ItemKind::Conversion);
    assert!(item.title.starts_with("6.21371"), "{item:?}");
    assert_eq!(item.subtitle, "10 km =");
    let Activation::CopyToClipboard(text) = l.activate(&item, "10 km in mi", NOW) else {
        panic!()
    };
    assert_eq!(text, item.title);
    // arithmetic without units stays the calculator's
    assert_eq!(
        l.query("10 * 3", NOW).sections[0].kind,
        SectionKind::Calculator
    );
}
