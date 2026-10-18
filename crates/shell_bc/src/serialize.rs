use crate::bytecode::{Bytecode, RedirTarget};

pub fn serialize(bc: &Bytecode) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();

    out.extend_from_slice(b"SHBC");
    out.push(0x03);
    out.extend_from_slice(&[0x00, 0x00, 0x00]);

    push_u32(&mut out, bc.const_pool.strings.len() as u32);
    for s in &bc.const_pool.strings {
        let b = s.as_bytes();
        push_u32(&mut out, b.len() as u32);
        out.extend_from_slice(b);
    }

    push_u32(&mut out, bc.redirs.len() as u32);
    for r in &bc.redirs {
        out.push(r.kind.to_u8());
        push_u32(&mut out, r.fd);
        match &r.target {
            RedirTarget::File(idx) => {
                out.push(1);
                push_u32(&mut out, *idx);
            }
            RedirTarget::Fd(n) => {
                out.push(2);
                push_u32(&mut out, *n);
            }
            RedirTarget::HereDoc(s) => {
                out.push(3);
                push_u32(&mut out, s.len() as u32);
                out.extend_from_slice(s.as_bytes());
            }
            RedirTarget::FilePath(_) => {
                out.push(1);
                push_u32(&mut out, 0);
            }
        }
    }

    push_u32(&mut out, bc.funcs.len() as u32);
    for f in &bc.funcs {
        let b = f.name.as_bytes();
        push_u32(&mut out, b.len() as u32);
        out.extend_from_slice(b);
        push_u32(&mut out, f.entry_ip);
    }

    push_u32(&mut out, bc.instructions.len() as u32);
    for instr in &bc.instructions {
        out.push(instr.op.to_u8());
        push_u32(&mut out, instr.operand);
    }

    // SLNE trailer: source-line metadata that older readers must ignore
    // (they stop after the instruction table; pool-seal copies it verbatim).
    out.extend_from_slice(b"SLNE");
    push_u32(&mut out, bc.redirs.len() as u32);
    for r in &bc.redirs {
        push_u32(&mut out, r.line);
    }
    push_u32(&mut out, bc.heredoc_warns.len() as u32);
    for w in &bc.heredoc_warns {
        push_u32(&mut out, w.line);
        push_u32(&mut out, w.eof_line);
        push_u32(&mut out, w.delim.len() as u32);
        out.extend_from_slice(w.delim.as_bytes());
    }
    push_u32(&mut out, bc.src.len() as u32);
    out.extend_from_slice(bc.src.as_bytes());

    out
}

fn push_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
