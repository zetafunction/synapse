#[macro_use]
extern crate error_chain;
#[macro_use]
extern crate prettytable;
#[macro_use]
extern crate serde_derive;

use synapse_rpc as rpc;
extern crate tungstenite as ws;

use rpc::criterion::Criterion;

mod client;
mod cmd;
mod config;
mod error;

use std::process;

use clap::{Arg, ArgAction, Command};
use error_chain::ChainedError;
use url::Url;

use self::client::Client;

fn main() {
    let config = config::load();
    let matches = Command::new("sycli")
        .about("cli interface for synapse")
        .author(env!("CARGO_PKG_AUTHORS"))
        .version(env!("CARGO_PKG_VERSION"))
        .subcommand_required(true)
        .arg(
            Arg::new("profile")
                .help("Profile to use when connecting to synapse.")
                .short('P')
                .long("profile")
                .default_value("default"),
        )
        .arg(
            Arg::new("server")
                .help("URI of the synapse client to connect to.")
                .short('s')
                .long("server"),
        )
        .arg(
            Arg::new("password")
                .help("Password to use when connecting to synapse.")
                .short('p')
                .long("password"),
        )
        .subcommands([
            Command::new("add")
                .about("Adds torrents to synapse.")
                .arg(
                    Arg::new("directory")
                        .help("Custom directory to download the torrent to.")
                        .short('d')
                        .long("directory"),
                )
                .arg(
                    Arg::new("pause")
                        .help("Whether or not the torrent should start paused.")
                        .short('P')
                        .long("pause")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("import")
                        .help("Whether or not the torrent should be imported.")
                        .short('i')
                        .long("import")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("files")
                        .help("Torrent files or magnets to add")
                        .required(true)
                        .index(1)
                        .action(ArgAction::Append),
                )
                .arg(
                    Arg::new("output")
                        .help("Output the results in the specified format.")
                        .short('o')
                        .long("output")
                        .value_parser(["json", "text"])
                        .default_value("text"),
                ),
            Command::new("del")
                .about("Deletes torrents from synapse.")
                .arg(
                    Arg::new("files")
                        .help("Delete files along with torrents.")
                        .short('f')
                        .long("files")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("torrents")
                        .help("Names of torrents to delete.")
                        .required(true)
                        .index(1)
                        .action(ArgAction::Append),
                ),
            Command::new("dl").about("Downloads a torrent.").arg(
                Arg::new("torrent")
                    .help("Name of torrent to download.")
                    .index(1)
                    .required(true),
            ),
            Command::new("file")
                .about("Manipulate a file.")
                .arg(
                    Arg::new("file id")
                        .help("ID of file to use.")
                        .index(1)
                        .required(true),
                )
                .subcommand_required(true)
                .subcommands([Command::new("priority")
                    .about("Adjust a file's priority.")
                    .arg(
                        Arg::new("file pri")
                            .help("priority to set file to (0-5)")
                            .index(1)
                            .required(true),
                    )]),
            Command::new("get")
                .about("Gets the specified resource.")
                .arg(
                    Arg::new("output")
                        .help("Output the results in the specified format.")
                        .short('o')
                        .long("output")
                        .value_parser(["json", "text"])
                        .default_value("text"),
                )
                .arg(
                    Arg::new("id")
                        .help("ID of the resource.")
                        .index(1)
                        .required(true),
                ),
            Command::new("list")
                .about("Lists resources of a given type in synapse.")
                .arg(
                    Arg::new("filter")
                        .help("Apply an array of json formatted criterion to the resources.")
                        .short('f')
                        .long("filter"),
                )
                .arg(
                    Arg::new("kind")
                        .help("The kind of resource to list.")
                        .value_parser(["torrent", "peer", "file", "server", "tracker", "piece"])
                        .default_value("torrent")
                        .short('k')
                        .long("kind"),
                )
                .arg(
                    Arg::new("output")
                        .help("Output the results in the specified format.")
                        .short('o')
                        .long("output")
                        .value_parser(["json", "text"])
                        .default_value("text"),
                ),
            Command::new("pause")
                .about("Pauses the given torrents.")
                .arg(
                    Arg::new("torrents")
                        .help("Names of torrents to pause.")
                        .required(true)
                        .index(1)
                        .action(ArgAction::Append),
                ),
            Command::new("resume")
                .about("Resumes the given torrents.")
                .arg(
                    Arg::new("torrents")
                        .help("Names of torrents to resume.")
                        .required(true)
                        .index(1)
                        .action(ArgAction::Append),
                ),
            Command::new("status").about("Server status"),
            Command::new("watch")
                .about("Watches the specified resource, printing out updates.")
                .arg(
                    Arg::new("output")
                        .help("Output the results in the specified format.")
                        .short('o')
                        .long("output")
                        .value_parser(["json", "text"])
                        .default_value("text"),
                )
                .arg(
                    Arg::new("completion")
                        .help("Polls until completion of torrent")
                        .short('c')
                        .long("completion")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("id")
                        .help("ID of the resource.")
                        .index(1)
                        .required(true),
                ),
            Command::new("torrent")
                .about("Manipulate torrent related resources")
                .arg(
                    Arg::new("torrent id")
                        .help("Name of torrent to download.")
                        .index(1),
                )
                .subcommand_required(true)
                .subcommands([
                    Command::new("move")
                        .about("Move a torrent to a new location")
                        .arg(
                            Arg::new("directory")
                                .help("Directory to move the torrent to.")
                                .index(1)
                                .required(true),
                        ),
                    Command::new("tracker")
                        .about("Manipulate trackers for a torrent")
                        .subcommand_required(true)
                        .subcommands([
                            Command::new("add").about("Add trackers to a torrent").arg(
                                Arg::new("uris")
                                    .help("URIs of trackers to add")
                                    .index(1)
                                    .required(true)
                                    .action(ArgAction::Append),
                            ),
                            Command::new("remove")
                                .about("Remove trackers from a torrent")
                                .arg(
                                    Arg::new("tracker id")
                                        .help("ids of trackers to remove")
                                        .index(1)
                                        .required(true)
                                        .action(ArgAction::Append),
                                ),
                            Command::new("announce")
                                .about("Announce to a tracker of a torrent")
                                .arg(
                                    Arg::new("tracker id")
                                        .help("ids of trackers to announce to")
                                        .index(1)
                                        .required(true)
                                        .action(ArgAction::Append),
                                ),
                        ]),
                    Command::new("peer")
                        .about("Manipulate peers for a torrent")
                        .subcommand_required(true)
                        .subcommands([
                            Command::new("add").about("Add peers to a torrent").arg(
                                Arg::new("peer ip")
                                    .help("IPs of peers to add")
                                    .index(1)
                                    .required(true)
                                    .action(ArgAction::Append),
                            ),
                            Command::new("remove")
                                .about("Remove peers from a torrent")
                                .arg(
                                    Arg::new("peer id")
                                        .help("ids of peers to remove")
                                        .index(1)
                                        .required(true)
                                        .action(ArgAction::Append),
                                ),
                        ]),
                    Command::new("tag")
                        .about("Manipulate tags for a torrent")
                        .subcommand_required(true)
                        .subcommands([
                            Command::new("add").about("Add tag to a torrent").arg(
                                Arg::new("tag names")
                                    .help("Name of tags to add")
                                    .index(1)
                                    .required(true)
                                    .action(ArgAction::Append),
                            ),
                            Command::new("remove")
                                .about("Remove tags from a torrent")
                                .arg(
                                    Arg::new("tag names")
                                        .help("Name of tags to remove")
                                        .index(1)
                                        .required(true)
                                        .action(ArgAction::Append),
                                ),
                        ]),
                    Command::new("priority")
                        .about("Change priority of a torrent")
                        .arg(
                            Arg::new("priority level")
                                .help("priority to set torrent to, 0-5")
                                .index(1)
                                .required(true),
                        ),
                    Command::new("trackers").about("Prints a torrent's trackers"),
                    Command::new("peers").about("Prints a torrent's peers"),
                    Command::new("tags").about("Prints a torrent's tags"),
                    Command::new("files").about("Prints a torrent's files"),
                    Command::new("verify").about("Verify integrity of downloaded files"),
                ])
                .arg(
                    Arg::new("output")
                        .help("Output the results in the specified format.")
                        .short('o')
                        .long("output")
                        .value_parser(["json", "text"])
                        .default_value("text"),
                ),
        ])
        .get_matches();

    let (mut server, mut pass) = match config.get(matches.get_one::<String>("profile").unwrap()) {
        Some(profile) => (profile.server.as_str(), profile.password.as_str()),
        None => {
            eprintln!(
                "Nonexistent profile {} referenced in argument!",
                matches.get_one::<String>("profile").unwrap()
            );
            process::exit(1);
        }
    };
    if let Some(url) = matches.get_one::<String>("server") {
        server = url;
    }
    if let Some(password) = matches.get_one::<String>("password") {
        pass = password;
    }
    let mut url = match Url::parse(server) {
        Ok(url) => url,
        Err(e) => {
            eprintln!("Server URL {} is not valid: {}", server, e);
            process::exit(1);
        }
    };
    url.query_pairs_mut().append_pair("password", pass);

    let client = match Client::new(url.clone()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "Failed to connect to synapse, ensure your URI and password are correct, {}",
                e.display_chain()
            );
            process::exit(1);
        }
    };

    if client.version().major != rpc::MAJOR_VERSION {
        eprintln!(
            "synapse RPC major version {} is not compatible with sycli RPC major version {}",
            client.version().major,
            rpc::MAJOR_VERSION
        );
        process::exit(1);
    }
    if client.version().minor < rpc::MINOR_VERSION {
        eprintln!(
            "synapse RPC minor version {} is not compatible with sycli RPC minor version {}",
            client.version().minor,
            rpc::MINOR_VERSION
        );
        process::exit(1);
    }

    if url.scheme() == "wss" {
        url.set_scheme("https").unwrap();
    } else {
        url.set_scheme("http").unwrap();
    }

    match matches.subcommand().unwrap() {
        ("add", add_args) => {
            let files = add_args
                .get_many("files")
                .unwrap()
                .map(String::as_str)
                .collect();
            let output = add_args.get_one::<String>("output").unwrap();
            let res = cmd::add(
                client,
                url.as_str(),
                files,
                add_args.get_one::<String>("directory").map(String::as_str),
                !add_args.get_flag("pause"),
                add_args.get_flag("import"),
                output,
            );
            if let Err(e) = res {
                eprintln!("Failed to add torrents: {}", e.display_chain());
                process::exit(1);
            }
        }
        ("del", del_args) => {
            let res = cmd::del(
                client,
                del_args
                    .get_many("torrents")
                    .unwrap()
                    .map(String::as_str)
                    .collect(),
                del_args.get_flag("files"),
            );
            if let Err(e) = res {
                eprintln!("Failed to delete torrents: {}", e.display_chain());
                process::exit(1);
            }
        }
        ("dl", dl_args) => {
            let res = cmd::dl(
                client,
                url.as_str(),
                dl_args.get_one::<String>("torrent").unwrap(),
            );
            if let Err(e) = res {
                eprintln!("Failed to download torrent: {}", e.display_chain());
                process::exit(1);
            }
        }
        ("file", file_args) => {
            let id = file_args.get_one::<String>("file id").unwrap();
            match file_args.subcommand().unwrap() {
                ("priority", priority_args) => {
                    let pri = priority_args.get_one::<String>("file pri").unwrap();
                    let res = cmd::set_file_pri(client, id, pri);
                    if let Err(e) = res {
                        eprintln!("Failed to download torrent: {}", e.display_chain());
                        process::exit(1);
                    }
                }
                _ => unreachable!(),
            }
        }
        ("get", get_args) => {
            let id = get_args
                .get_one::<String>("id")
                .unwrap()
                .to_ascii_uppercase();
            let output = get_args.get_one::<String>("output").unwrap();
            let res = cmd::get(client, &id, output);
            if let Err(e) = res {
                eprintln!("Failed to get resource: {}", e.display_chain());
                process::exit(1);
            }
        }
        ("list", list_args) => {
            let crit = if let Some(searches) = list_args.get_one::<String>("filter") {
                parse_filter(searches)
            } else {
                Vec::new()
            };

            let kind = list_args.get_one::<String>("kind").unwrap();
            let output = list_args.get_one::<String>("output").unwrap();
            let res = cmd::list(client, kind, crit, output);
            if let Err(e) = res {
                eprintln!("Failed to list torrents: {}", e.display_chain());
                process::exit(1);
            }
        }
        ("pause", pause_args) => {
            let res = cmd::pause(
                client,
                pause_args
                    .get_many("torrents")
                    .unwrap()
                    .map(String::as_str)
                    .collect(),
            );
            if let Err(e) = res {
                eprintln!("Failed to pause torrents: {}", e.display_chain());
                process::exit(1);
            }
        }
        ("resume", resume_args) => {
            let res = cmd::resume(
                client,
                resume_args
                    .get_many("torrents")
                    .unwrap()
                    .map(String::as_str)
                    .collect(),
            );
            if let Err(e) = res {
                eprintln!("Failed to resume torrents: {}", e.display_chain());
                process::exit(1);
            }
        }
        ("status", _) => {
            if let Err(e) = cmd::status(client) {
                eprintln!("Failed to get server status: {}", e.display_chain());
                process::exit(1);
            }
        }
        ("torrent", torrent_args) => {
            let id = torrent_args
                .get_one::<String>("torrent id")
                .map(String::as_str)
                .map_or("none".to_string(), str::to_ascii_uppercase);
            let output = torrent_args.get_one::<String>("output").unwrap();
            match torrent_args.subcommand().unwrap() {
                ("move", move_args) => {
                    let dir = move_args.get_one::<String>("directory").unwrap();
                    if let Err(e) = cmd::move_torrent(client, &id, dir) {
                        eprintln!("Failed to move torrent: {}", e.display_chain());
                        process::exit(1);
                    }
                }
                ("verify", _) => {
                    if let Err(e) = cmd::verify_torrent(client, &id) {
                        eprintln!("Failed to verify integrity: {}", e.display_chain());
                        process::exit(1);
                    }
                }
                ("tracker", tracker_args) => match tracker_args.subcommand().unwrap() {
                    ("add", add_args) => {
                        if let Err(e) = cmd::add_trackers(
                            client,
                            &id,
                            add_args
                                .get_many("uris")
                                .unwrap()
                                .map(String::as_str)
                                .collect(),
                        ) {
                            eprintln!("Failed to add trackers: {}", e.display_chain());
                            process::exit(1);
                        }
                    }
                    ("remove", remove_args) => {
                        if let Err(e) = cmd::remove_trackers(
                            client,
                            remove_args
                                .get_many("tracker id")
                                .unwrap()
                                .map(String::as_str)
                                .collect(),
                        ) {
                            eprintln!("Failed to remove trackers: {}", e.display_chain());
                            process::exit(1);
                        }
                    }
                    ("announce", announce_args) => {
                        if let Err(e) = cmd::announce_trackers(
                            client,
                            announce_args
                                .get_many("tracker id")
                                .unwrap()
                                .map(String::as_str)
                                .collect(),
                        ) {
                            eprintln!("Failed to remove trackers: {}", e.display_chain());
                            process::exit(1);
                        }
                    }
                    _ => unreachable!(),
                },
                ("peer", peer_args) => match peer_args.subcommand().unwrap() {
                    ("add", add_args) => {
                        if let Err(e) = cmd::add_peers(
                            client,
                            &id,
                            add_args
                                .get_many("peer ip")
                                .unwrap()
                                .map(String::as_str)
                                .collect(),
                        ) {
                            eprintln!("Failed to add peers: {}", e.display_chain());
                            process::exit(1);
                        }
                    }
                    ("remove", remove_args) => {
                        if let Err(e) = cmd::remove_peers(
                            client,
                            remove_args
                                .get_many("peer id")
                                .unwrap()
                                .map(String::as_str)
                                .collect(),
                        ) {
                            eprintln!("Failed to remove peers: {}", e.display_chain());
                            process::exit(1);
                        }
                    }
                    _ => unreachable!(),
                },
                ("tag", tag_args) => match tag_args.subcommand().unwrap() {
                    ("add", add_args) => {
                        if let Err(e) = cmd::add_tags(
                            client,
                            &id,
                            add_args
                                .get_many("tag names")
                                .unwrap()
                                .map(String::as_str)
                                .collect(),
                        ) {
                            eprintln!("Failed to add peers: {}", e.display_chain());
                            process::exit(1);
                        }
                    }
                    ("remove", remove_args) => {
                        if let Err(e) = cmd::remove_tags(
                            client,
                            &id,
                            remove_args
                                .get_many("tag names")
                                .unwrap()
                                .map(String::as_str)
                                .collect(),
                        ) {
                            eprintln!("Failed to remove peers: {}", e.display_chain());
                            process::exit(1);
                        }
                    }
                    _ => unreachable!(),
                },
                ("priority", priority_args) => {
                    let pri = priority_args.get_one::<String>("priority level").unwrap();
                    if let Err(e) = cmd::set_torrent_pri(client, &id, pri) {
                        eprintln!("Failed to set torrent priority: {}", e.display_chain());
                        process::exit(1);
                    }
                }
                ("files", _) => {
                    if let Err(e) = cmd::get_files(client, &id, output) {
                        eprintln!("Failed to get torrent files: {}", e.display_chain());
                        process::exit(1);
                    }
                }
                ("peers", _) => {
                    if let Err(e) = cmd::get_peers(client, &id, output) {
                        eprintln!("Failed to get torrent peers: {}", e.display_chain());
                        process::exit(1);
                    }
                }
                ("tags", _) => {
                    if let Err(e) = cmd::get_tags(client, &id) {
                        eprintln!("Failed to get torrent tags: {}", e.display_chain());
                        process::exit(1);
                    }
                }
                ("trackers", _) => {
                    if let Err(e) = cmd::get_trackers(client, &id, output) {
                        eprintln!("Failed to get torrent trackers: {}", e.display_chain());
                        process::exit(1);
                    }
                }
                _ => unreachable!(),
            }
        }
        ("watch", watch_args) => {
            let id = watch_args
                .get_one::<String>("id")
                .unwrap()
                .to_ascii_uppercase();
            let output = watch_args.get_one::<String>("output").unwrap();
            let completion = watch_args.get_flag("completion");
            let res = cmd::watch(client, &id, output, completion);
            if let Err(e) = res {
                eprintln!("Failed to watch resource: {}", e.display_chain());
                process::exit(1);
            }
        }
        _ => {}
    }
}

/// Parse search criteria out of a filter string
fn parse_filter(searches: &str) -> Vec<Criterion> {
    use regex::Regex;
    use rpc::criterion::{Operation, Value};

    // return vector to hold found criterion
    let mut criterion = Vec::new();

    // regular expression for finding search criteria that take string types
    let string_searches = Regex::new(
        r#"(?x)
        \b(name|path|status|tracker) # field name
        (==|!=|::|:)                 # delimiter
        ("(.+?)"                     # quoted argument
        |([0-9.a-zA-Z]+))            # unquoted argument
        "#,
    )
    .unwrap();

    // regular expression for finding search criteria that take numeric types
    let numeric_searches = Regex::new(
        r#"(?x)
        \b(size|progress|priority|availability
           |rate_up|rate_down|throttle_up|throttle_down
           |transferred_up|transferred_down
           |peers|trackers|files)    # field name
        (>=|<=|==|!=|>|<)            # delimiter
        ("([0-9.]+?)"                # quoted argument
        |([0-9.]+))                  # unquoted argument
        "#,
    )
    .unwrap();

    // find all string like searches and add to criterion
    for cap in string_searches.captures_iter(searches) {
        let field = cap[1].to_string();
        let op = match &cap[2] {
            "==" => Operation::Eq,
            "!=" => Operation::Neq,
            "::" => Operation::Like,
            ":" => Operation::ILike,
            _ => unreachable!(),
        };
        let arg = if let Some(quoted) = cap.get(4) {
            quoted
        } else {
            // if quoted arg did not match, an unquoted arg must have matched
            cap.get(5).unwrap()
        }
        .as_str();
        let value = Value::S(arg.to_string());
        criterion.push(Criterion { field, op, value });
    }

    // find all numeric searches and add to criterion
    for cap in numeric_searches.captures_iter(searches) {
        let field = cap[1].to_string();
        let op = match &cap[2] {
            ">=" => Operation::GTE,
            "<=" => Operation::LTE,
            "==" => Operation::Eq,
            "!=" => Operation::Neq,
            ">" => Operation::GT,
            "<" => Operation::LT,
            _ => unreachable!(),
        };
        let arg = if let Some(quoted) = cap.get(4) {
            quoted
        } else {
            // if quoted arg did not match, an unquoted arg must have matched
            cap.get(5).unwrap()
        }
        .as_str();
        let value = Value::F(arg.parse().expect("Invalid numeric value"));
        criterion.push(Criterion { field, op, value });
    }

    // if no matches found, assume a simple name query
    if criterion.is_empty() {
        criterion.push(Criterion {
            field: "name".to_string(),
            op: Operation::ILike,
            value: Value::S(searches.to_string()),
        });
    }

    criterion
}

#[cfg(test)]
mod tests {
    use super::*;
    use rpc::criterion::{Operation, Value};

    #[test]
    fn parse_filter_simple() {
        let name_query = vec![Criterion {
            field: "name".to_string(),
            op: Operation::ILike,
            value: Value::S("abcd".to_string()),
        }];
        assert_eq!(parse_filter("abcd"), name_query.clone());
        assert_eq!(parse_filter("name:abcd"), name_query);
    }

    #[test]
    fn parse_filter_simple_with_space() {
        let name_query = vec![Criterion {
            field: "name".to_string(),
            op: Operation::ILike,
            value: Value::S("abcd efgh ijkl".to_string()),
        }];
        assert_eq!(parse_filter("abcd efgh ijkl"), name_query);
    }

    #[test]
    fn parse_filter_case_sensitive() {
        let name_query = vec![Criterion {
            field: "path".to_string(),
            op: Operation::Like,
            value: Value::S("ISOs Directory".to_string()),
        }];
        assert_eq!(parse_filter(r#"path::"ISOs Directory""#), name_query);
    }

    #[test]
    fn parse_filter_quoted_with_space() {
        let name_query = vec![Criterion {
            field: "path".to_string(),
            op: Operation::ILike,
            value: Value::S("/Linux ISOs/".to_string()),
        }];
        assert_eq!(parse_filter(r#"path:"/Linux ISOs/""#), name_query);
    }

    #[test]
    fn parse_filter_bad_field_name() {
        let name_query = vec![Criterion {
            field: "name".to_string(),
            op: Operation::ILike,
            value: Value::S("badfield==4".to_string()),
        }];
        assert_eq!(parse_filter("badfield==4"), name_query);
    }

    #[test]
    fn parse_filter_bad_delimeter_after_valid() {
        let name_query = vec![Criterion {
            field: "name".to_string(),
            op: Operation::ILike,
            value: Value::S("foo".to_string()),
        }];
        assert_eq!(parse_filter("name:foo key~val"), name_query);
    }

    #[test]
    fn parse_filter_bad_field_name_after_valid() {
        let name_query = vec![Criterion {
            field: "name".to_string(),
            op: Operation::ILike,
            value: Value::S("foo".to_string()),
        }];
        assert_eq!(parse_filter("name:foo badfield==4"), name_query);
    }

    #[test]
    fn parse_filter_numbers() {
        let gt_query = vec![Criterion {
            field: "transferred_up".to_string(),
            op: Operation::GT,
            value: Value::F(500.23),
        }];
        assert_eq!(parse_filter("transferred_up>500.23"), gt_query);

        let gte_query = vec![Criterion {
            field: "transferred_up".to_string(),
            op: Operation::GTE,
            value: Value::F(500.23),
        }];
        assert_eq!(parse_filter("transferred_up>=500.23"), gte_query);
    }

    #[test]
    fn parse_filter_multi_query() {
        let multi_query = vec![
            Criterion {
                field: "transferred_up".to_string(),
                op: Operation::GT,
                value: Value::F(500.23),
            },
            Criterion {
                field: "tracker".to_string(),
                op: Operation::ILike,
                value: Value::S("debian".to_string()),
            },
            Criterion {
                field: "priority".to_string(),
                op: Operation::Eq,
                value: Value::F(4.0),
            },
        ];
        let p = parse_filter("transferred_up>500.23 tracker:debian priority==4.0");
        assert_eq!(p.len(), multi_query.len());
        for q in &multi_query {
            assert!(p.contains(&q));
        }
    }
}
