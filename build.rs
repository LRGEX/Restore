fn main() {
    if cfg!(target_os = "windows") {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/app-icon.ico");

        // Version metadata — reduces Defender heuristic false positives.
        // Unsigned exes with no metadata look maximally suspicious.
        let v: Vec<u64> = env!("CARGO_PKG_VERSION")
            .split('.')
            .map(|s| s.parse().unwrap_or(0))
            .collect();
        let major = *v.get(0).unwrap_or(&1);
        let minor = *v.get(1).unwrap_or(&0);
        let patch = *v.get(2).unwrap_or(&0);
        let packed = (major << 48) | (minor << 32) | (patch << 16);
        res.set_version_info(winres::VersionInfo::FILEVERSION, packed);
        res.set_version_info(winres::VersionInfo::PRODUCTVERSION, packed);
        res.set("CompanyName", "LRGEX");
        res.set("ProductName", "LRGEX Restore");
        res.set("FileDescription", "Folder backup and path-aware restore utility");
        res.set("LegalCopyright", "Copyright (c) 2025 LRGEX");
        res.set("OriginalFilename", "LRGEXRestore.exe");

        let _ = res.compile();
    }
    println!("cargo:rerun-if-changed=signing.pub");
}
