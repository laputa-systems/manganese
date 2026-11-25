use prost_types::field_descriptor_proto::Type;
use std::error::Error;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let source = PathBuf::from(args.next().ok_or("missing Chromium source directory")?);
    let output = PathBuf::from(args.next().ok_or("missing output directory")?);
    if args.next().is_some() { return Err("unexpected argument".into()); }
    let paths = [
        source.join("components/leveldb_proto/internal/proto/shared_db_metadata.proto"),
        source.join("components/origin_trials/proto/db_trial_token.proto"),
        source.join("content/browser/background_sync/background_sync.proto"),
        source.join("content/browser/payments/payment_app.proto"),
    ];
    let mut descriptors = protox::compile(paths, [source])?;
    let mut changed = 0;
    for file in &mut descriptors.file {
        if !matches!(file.package(), "origin_trials_pb" | "content") { continue; }
        for message in &mut file.message_type {
            for field in &mut message.field {
                if field.r#type() == Type::String {
                    field.r#type = Some(Type::Bytes as i32);
                    changed += 1;
                }
            }
        }
    }
    if changed != 19 { return Err("unexpected proto2 string fields".into()); }
    std::fs::create_dir_all(&output)?;
    prost_build::Config::new().out_dir(&output).disable_comments(["."]).compile_fds(descriptors)?;
    for file in ["leveldb_proto.rs", "origin_trials_pb.rs", "content.rs"] {
        let path = output.join(file);
        let generated = std::fs::read_to_string(&path)?;
        let contents: String = generated.lines().filter(|line| !line.trim_start().starts_with("//"))
            .map(|line| format!("{line}\n")).collect();
        std::fs::write(path, contents)?;
    }
    Ok(())
}
