#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    if bytes.len() > 1 << 20 {
        return;
    }
    let source = powerio_core::Source::from_memory("case.glm", bytes).unwrap();
    if let Ok(raw) = powerio_dist::glm::parse_raw_source(source) {
        let mut stack = vec![raw.statements.as_slice()];
        while let Some(statements) = stack.pop() {
            for statement in statements {
                use powerio_dist::glm::Statement;
                let tokens = match statement {
                    Statement::Simple { tokens, .. } => tokens.as_slice(),
                    Statement::Block { header, body, .. } => {
                        stack.push(body);
                        header.as_slice()
                    }
                    Statement::Directive(token) => std::slice::from_ref(token),
                    _ => unreachable!(),
                };
                for token in tokens {
                    let _ = (token.text(), token.span());
                }
            }
        }
    }
});
