// Embute pacote/icone/fss.ico no executável (recurso de ícone número 1), para
// o Explorer, a barra de tarefas e o atalho mostrarem o ícone do FSS.
// Incluído pelo build.rs do fluxo, do fluxo-host e do fluxo-client:
//     include!("../../pacote/icone/recurso.rs");
// Usa o rc.exe do Windows SDK (o mesmo que vem com o Visual Studio). Sem ele,
// ou fora do Windows/MSVC, o programa compila sem ícone e o cargo avisa.

fn embutir_icone() {
    let raiz = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icone = raiz.join("../../pacote/icone/fss.ico");
    println!("cargo:rerun-if-changed={}", icone.display());
    println!("cargo:rerun-if-changed=../../pacote/icone/recurso.rs");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !windows || !msvc {
        return;
    }
    let Some(rc) = achar_rc() else {
        println!("cargo:warning=rc.exe (Windows SDK) não encontrado: o programa fica sem ícone");
        return;
    };
    let saida = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let script = saida.join("icone.rc");
    let caminho = icone
        .canonicalize()
        .expect("pacote/icone/fss.ico (cargo run -p fss-icone)")
        .display()
        .to_string()
        .trim_start_matches(r"\\?\")
        .replace('\\', "\\\\");
    std::fs::write(&script, format!("1 ICON \"{caminho}\"\n")).unwrap();
    let res = saida.join("icone.res");
    let ok = std::process::Command::new(&rc)
        .args(["/nologo", "/fo"])
        .arg(&res)
        .arg(&script)
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        println!("cargo:rustc-link-arg-bins={}", res.display());
    } else {
        println!("cargo:warning=o rc.exe falhou: o programa fica sem ícone");
    }
}

/// O rc.exe: o do PATH ou o da versão mais nova do Windows SDK.
fn achar_rc() -> Option<std::path::PathBuf> {
    if std::process::Command::new("rc.exe").arg("/?").output().is_ok() {
        return Some("rc.exe".into());
    }
    let arq = if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") { "arm64" } else { "x64" };
    let base = std::path::Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versoes: Vec<_> = std::fs::read_dir(base)
        .ok()?
        .flatten()
        .map(|e| e.path().join(arq).join("rc.exe"))
        .filter(|p| p.exists())
        .collect();
    versoes.sort();
    versoes.pop()
}
