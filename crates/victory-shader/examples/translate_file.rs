//! Translates one shader container file (a console compiler's output, say)
//! and prints whether it translates and what it binds.
//!
//!   cargo run --release -p victory-shader --example translate_file -- FILE

fn main() {
    let path = std::env::args().nth(1).expect("FILE");
    let bytes = std::fs::read(&path).expect("read");
    match victory_shader::Container::parse(&bytes) {
        Ok(c) => {
            let parts: Vec<String> = c.parts.iter().map(|p| format!("{:?} ({} bytes)", p.fourcc, p.data.len())).collect();
            println!("parts: {}", parts.join(", "));
        }
        Err(e) => {
            println!("not a container: {e}");
            return;
        }
    }
    match victory_shader::emit::translate(&bytes) {
        Ok(s) => println!("translates: {} bytes of DXIL, {} user-data dwords", s.dxil.len(), s.user_data.len()),
        Err(e) => println!("does not translate: {e}"),
    }
}
