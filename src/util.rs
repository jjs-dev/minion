use rand::seq::SliceRandom;

const ID_CHARS: &[u8] = b"qwertyuiopasdfghjklzxcvbnm1234567890";
const ID_SIZE: usize = 8;

pub(crate) fn gen_jail_id() -> String {
    let mut gen = rand::thread_rng();
    let mut out = Vec::new();
    for _i in 0..ID_SIZE {
        let ch = *(ID_CHARS.choose(&mut gen).unwrap());
        out.push(ch);
    }
    String::from_utf8_lossy(&out[..]).to_string()
}
