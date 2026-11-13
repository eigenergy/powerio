//! Explicit collection inspection and independent selected-case results.
use clap::Subcommand;
use powerio::collection::CollectionOperation;
use powerio::{Collection, CollectionLimits, CollectionSelection, ParseOptions};
use serde_json::json;
use std::path::{Path, PathBuf};

#[derive(Debug, Subcommand)]
pub enum CollectionCommand {
    /// List all inventory members and suggested selections as JSON.
    Inspect { input: PathBuf },
    /// Parse explicit entries or one directory/document group. Print one JSON
    /// result per selection; an invalid case does not prevent its neighbors.
    Parse {
        input: PathBuf,
        /// Primary paths relative to --root, in output order. Repeat for batches.
        #[arg(long, conflicts_with = "member")]
        entry: Vec<String>,
        /// Permitted root within the collection.
        #[arg(long)]
        root: Option<String>,
        /// Explicit parser token; required for directory and document groups.
        #[arg(long)]
        from: Option<String>,
        /// Explicit document group member relative to --root. Repeat as needed.
        #[arg(long, conflicts_with = "entry")]
        member: Vec<String>,
        /// Read PowerIO IR rather than a grid exchange format.
        #[arg(long, conflicts_with_all=["from","member"])]
        deserialize: bool,
        /// Serialize successful modules as case-0001.pio.json, etc.
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[command(flatten)]
        native: Box<super::SincalCliOptions>,
    },
}

pub fn run(command: CollectionCommand) -> anyhow::Result<()> {
    match command {
        CollectionCommand::Inspect { input } => {
            let collection = Collection::open(&input, CollectionLimits::default())?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"entries":collection.entries(),"candidates":collection.inspect()?})
                )?
            );
            Ok(())
        }
        CollectionCommand::Parse {
            input,
            entry,
            root,
            from,
            member,
            deserialize,
            output,
            native,
        } => {
            let collection = Collection::open(&input, CollectionLimits::default())?;
            let native_format = from.as_deref().and_then(|name| match name {
                "sincal-balanced" => Some(super::FormatArg::SincalBalanced),
                "sincal-multiconductor" => Some(super::FormatArg::SincalMulticonductor),
                _ => None,
            });
            let mut options = native.options(native_format)?;
            options.format = None;
            let operation = if deserialize {
                CollectionOperation::Deserialize
            } else {
                CollectionOperation::Parse
            };
            let mut base = CollectionSelection::default();
            base.root = root;
            base.format = from;
            base.operation = operation;
            base.members = (!member.is_empty()).then_some(member);
            let selections: Vec<_> = if entry.is_empty() {
                vec![base]
            } else {
                entry
                    .into_iter()
                    .map(|primary| {
                        let mut selection = base.clone();
                        selection.primary = Some(primary);
                        selection
                    })
                    .collect()
            };
            run_selections(&collection, &selections, &options, output.as_deref())
        }
    }
}
fn run_selections(
    collection: &Collection,
    selections: &[CollectionSelection],
    options: &ParseOptions,
    output: Option<&Path>,
) -> anyhow::Result<()> {
    let mut failed = false;
    let mut results = Vec::new();
    for (index, selection) in selections.iter().enumerate() {
        let id = format!("case-{:04}", index + 1);
        match collection.parse(selection, options) {
            Ok(parsed) => {
                let written = output
                    .map(|root| {
                        powerio::serialize(&parsed.module, root.join(format!("{id}.pio.json")))
                    })
                    .transpose();
                match written {
                    Ok(_) => {
                        failed |= parsed.incomplete;
                        results.push(json!({"id":id,"selection":selection,"status":if parsed.incomplete {"incomplete"} else {"parsed"},"value_type":parsed.module.value().type_name(),"dependencies":parsed.dependencies,"diagnostics":parsed.module.diagnostics()}));
                    }
                    Err(error) => {
                        failed = true;
                        results.push(json!({"id":id,"selection":selection,"status":"error","error":error.to_string()}));
                    }
                }
            }
            Err(error) => {
                failed = true;
                results.push(json!({"id":id,"selection":selection,"status":"error","error":error.to_string(),"diagnostics":error.diagnostics()}));
            }
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"results":results}))?
    );
    anyhow::ensure!(
        !failed,
        "one or more collection selections failed or reported incomplete dependencies"
    );
    Ok(())
}
