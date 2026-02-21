// Wraps decrypt.wasm: HMAC verification and AES-GCM video URL decryption for bytegooty.com.

use anyhow::{Result, anyhow};
use wasmtime::*;

pub struct WasmDecrypt {
    store: Store<()>,
    memory: Memory,
    wasm_alloc: TypedFunc<i32, i32>,
    wasm_free: TypedFunc<i32, ()>,
    wasm_hmac_verify: TypedFunc<(i32, i32, i32), i32>,
    wasm_decrypt: TypedFunc<(i32, i32, i32, i32), i32>,
}

impl WasmDecrypt {
    /// Instantiate the module from raw bytes.
    pub fn new(engine: &Engine, wasm_bytes: &[u8]) -> Result<Self> {
        let module = Module::new(engine, wasm_bytes)?;
        let mut store = Store::new(engine, ());
        let instance = Instance::new(&mut store, &module, &[])?;

        // Call WASI-style _initialize if the module exports it.
        if let Ok(init) = instance.get_typed_func::<(), ()>(&mut store, "_initialize") {
            let _ = init.call(&mut store, ());
        }

        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| anyhow!("wasm export 'memory' not found"))?;
        let wasm_alloc = instance.get_typed_func::<i32, i32>(&mut store, "wasm_alloc")?;
        let wasm_free = instance.get_typed_func::<i32, ()>(&mut store, "wasm_free")?;
        let wasm_hmac_verify =
            instance.get_typed_func::<(i32, i32, i32), i32>(&mut store, "wasm_hmac_verify")?;
        let wasm_decrypt =
            instance.get_typed_func::<(i32, i32, i32, i32), i32>(&mut store, "wasm_decrypt")?;

        Ok(Self {
            store,
            memory,
            wasm_alloc,
            wasm_free,
            wasm_hmac_verify,
            wasm_decrypt,
        })
    }

    // * private memory helpers

    /// Allocate a null-terminated C-string in wasm memory; returns its pointer.
    fn write_str(&mut self, s: &str) -> Result<i32> {
        let bytes = s.as_bytes();
        let ptr = self
            .wasm_alloc
            .call(&mut self.store, bytes.len() as i32 + 1)?;
        self.memory.write(&mut self.store, ptr as usize, bytes)?;
        self.memory
            .write(&mut self.store, ptr as usize + bytes.len(), &[0])?;
        Ok(ptr)
    }

    /// Read a null-terminated C-string from wasm memory.
    fn read_str(&mut self, ptr: i32) -> Result<String> {
        let mut end = ptr as usize;
        loop {
            let mut b = [0u8];
            self.memory.read(&self.store, end, &mut b)?;
            if b[0] == 0 {
                break;
            }
            end += 1;
        }
        let len = end - ptr as usize;
        let mut bytes = vec![0u8; len];
        self.memory.read(&self.store, ptr as usize, &mut bytes)?;
        Ok(String::from_utf8(bytes)?)
    }

    fn free_ptr(&mut self, ptr: i32) -> Result<()> {
        self.wasm_free.call(&mut self.store, ptr)?;
        Ok(())
    }

    // * public API

    /// Verify the per-session HMAC using wasm_hmac_verify.
    pub fn hmac_verify(&mut self, code: &str, secret: &str, fragment: &str) -> Result<bool> {
        let p_code = self.write_str(code)?;
        let p_sec = self.write_str(secret)?;
        let p_frag = self.write_str(fragment)?;
        let ok = self
            .wasm_hmac_verify
            .call(&mut self.store, (p_code, p_sec, p_frag))?;
        self.free_ptr(p_code)?;
        self.free_ptr(p_sec)?;
        self.free_ptr(p_frag)?;
        Ok(ok == 1)
    }

    /// Decrypt the base-64 encoded video URL using wasm_decrypt.
    pub fn decrypt(
        &mut self,
        enc_b64: &str,
        code: &str,
        secret: &str,
        now_ts: i32,
    ) -> Result<String> {
        let p_enc = self.write_str(enc_b64)?;
        let p_code = self.write_str(code)?;
        let p_sec = self.write_str(secret)?;
        let p_res = self
            .wasm_decrypt
            .call(&mut self.store, (p_enc, p_code, p_sec, now_ts))?;
        let result = self.read_str(p_res)?;
        self.free_ptr(p_enc)?;
        self.free_ptr(p_code)?;
        self.free_ptr(p_sec)?;
        Ok(result)
    }
}
