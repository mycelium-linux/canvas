fn main() { for app in ::canvas::spore_catalog::scan() { println!("{} {} -> {}", app.name, app.version, app.workspace); } }
