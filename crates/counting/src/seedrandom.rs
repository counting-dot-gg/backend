pub struct SeedRandom {
    i: u8,
    j: u8,
    s: [u8; 256],
}

impl SeedRandom {
    pub fn new(seed: &str) -> Self {
        let units: Vec<u16> = seed.encode_utf16().collect();
        let mut key: Vec<i32> = Vec::new();
        let mut smear: i32 = 0;
        for (j, &u) in units.iter().enumerate() {
            let k = j & 255;
            let prev = key.get(k).copied().unwrap_or(0);
            smear ^= prev.wrapping_mul(19);
            let v = smear.wrapping_add(u as i32) & 255;
            if k < key.len() {
                key[k] = v;
            } else {
                key.push(v);
            }
        }
        if key.is_empty() {
            key.push(0);
        }
        let mut s = [0u8; 256];
        for (i, x) in s.iter_mut().enumerate() {
            *x = i as u8;
        }
        let mut j: u8 = 0;
        for i in 0..256 {
            let t = s[i];
            j = j.wrapping_add(key[i % key.len()] as u8).wrapping_add(t);
            s[i] = s[j as usize];
            s[j as usize] = t;
        }
        let mut r = SeedRandom { i: 0, j: 0, s };
        r.g(256);
        r
    }

    fn g(&mut self, count: usize) -> f64 {
        let mut r = 0.0;
        for _ in 0..count {
            self.i = self.i.wrapping_add(1);
            let t = self.s[self.i as usize];
            self.j = self.j.wrapping_add(t);
            self.s[self.i as usize] = self.s[self.j as usize];
            self.s[self.j as usize] = t;
            let idx = self.s[self.i as usize].wrapping_add(t);
            r = r * 256.0 + self.s[idx as usize] as f64;
        }
        r
    }

    pub fn next_f64(&mut self) -> f64 {
        let significance = 2f64.powi(52);
        let overflow = significance * 2.0;
        let mut n = self.g(6);
        let mut d = 256f64.powi(6);
        let mut x = 0.0f64;
        while n < significance {
            n = (n + x) * 256.0;
            d *= 256.0;
            x = self.g(1);
        }
        while n >= overflow {
            n /= 2.0;
            d /= 2.0;
            x = ((x as u32) >> 1) as f64;
        }
        (n + x) / d
    }
}
