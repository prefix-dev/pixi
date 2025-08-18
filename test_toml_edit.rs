use toml_edit::{DocumentMut, Item, Table, value};

fn main() {
    let mut doc = DocumentMut::new();
    
    // Test approach 1: Direct dotted key
    println!("=== Approach 1: Direct dotted key ===");
    let mut doc1 = doc.clone();
    doc1["envs"]["sdl.example"]["channels"] = value(vec!["conda-forge"]);
    println!("{}", doc1);
    
    // Test approach 2: Using nested table creation
    println!("\n=== Approach 2: Nested table creation ===");
    let mut doc2 = doc.clone();
    let envs_table = doc2.entry("envs").or_insert_with(|| Item::Table(Table::new()));
    let env_table = envs_table.as_table_mut().unwrap()
        .entry("sdl.example").or_insert_with(|| Item::Table(Table::new()));
    env_table.as_table_mut().unwrap()["channels"] = value(vec!["conda-forge"]);
    println!("{}", doc2);
    
    // Test approach 3: Creating table with explicit structure
    println!("\n=== Approach 3: Explicit table structure ===");
    let mut doc3 = doc.clone();
    doc3.as_table_mut().insert("envs", Item::Table(Table::new()));
    let envs = doc3["envs"].as_table_mut().unwrap();
    envs.insert("sdl.example", Item::Table(Table::new()));
    let env = envs["sdl.example"].as_table_mut().unwrap();
    env.insert("channels", value(vec!["conda-forge"]));
    println!("{}", doc3);
}