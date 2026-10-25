#!/usr/bin/env python3
import argparse
import fcntl
import json
import re
import hashlib
import os
from pathlib import Path
import shutil
import subprocess

def tree_digest(directory):
    digest = hashlib.sha256()
    for path in sorted(directory.rglob("*")):
        if path.is_file() and path.name != ".draw-benchmark-source":
            digest.update(str(path.relative_to(directory)).encode())
            digest.update(path.read_bytes())
    return digest.hexdigest()


def replace_once(path, before, after):
    text = path.read_text()
    if text.count(before) != 1:
        raise SystemExit(f"Pinned instrumentation anchor changed in {path}: {before!r}")
    path.write_text(text.replace(before, after))


def prepare(source, scratch):
    identity = hashlib.sha256(Path(__file__).read_bytes() + tree_digest(source).encode()).hexdigest()
    marker = scratch / ".draw-benchmark-source"
    if marker.exists():
        try:
            saved = json.loads(marker.read_text())
        except (ValueError, OSError):
            saved = None
        if saved == {"input": identity, "output": tree_digest(scratch)}:
            return identity
    if scratch.exists():
        shutil.rmtree(scratch)
    shutil.copytree(source, scratch)
    element = scratch / "src/element.rs"
    element.write_text(element.read_text() + """
    thread_local! {
        pub(crate) static DRAW_BENCH_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        pub(crate) static DRAW_BENCH_ELEMENTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }
    """)
    replace_once(element,
        "        let element = with_element_arena(|arena| arena.alloc(|| Drawable::new(element)))",
        "        if DRAW_BENCH_ENABLED.with(|enabled| enabled.get()) { DRAW_BENCH_ELEMENTS.with(|count| count.set(count.get() + 1)); }\n        let element = with_element_arena(|arena| arena.alloc(|| Drawable::new(element)))")
    window = scratch / "src/window.rs"
    replace_once(window,
        "    pub fn draw(&mut self, cx: &mut App) -> ArenaClearNeeded {",
        "    pub fn draw(&mut self, cx: &mut App) -> ArenaClearNeeded {\n        crate::element::DRAW_BENCH_ELEMENTS.with(|count| count.set(0));")
    window.write_text(window.read_text() + """
    impl Window {
        #[allow(missing_docs)]
        pub fn draw_benchmark_equality_probes(&mut self) {
            use crate::AtlasBackend;
            let original_background = self.rendered_frame.scene.paths[0].color;
            self.rendered_frame.scene.paths[0].color = crate::linear_gradient(90.,
                crate::linear_color_stop(crate::rgb(0xff0000), 0.),
                crate::linear_color_stop(crate::rgb(0x0000ff), 1.));
            let before = self.draw_benchmark_scene().5;
            let old_space = self.rendered_frame.scene.paths[0].color.color_space;
            self.rendered_frame.scene.paths[0].color.color_space = match old_space {
                crate::ColorSpace::Srgb => crate::ColorSpace::Oklab,
                crate::ColorSpace::Oklab => crate::ColorSpace::Srgb,
            };
            assert_ne!(before, self.draw_benchmark_scene().5, "color-space mutation escaped digest");
            self.rendered_frame.scene.paths[0].color.color_space = old_space;
            let tile = self.rendered_frame.scene.monochrome_sprites[0].tile;
            let key = (tile.texture_id.index, tile.tile_id.0);
            let saved = crate::platform::DRAW_BENCH_TILES.with(|tiles| tiles.borrow().clone());
            let mut backend = crate::HeadlessAtlasBackend::default();
            let a = backend.insert(tile.texture_id.kind, crate::size(crate::DevicePixels(2), crate::DevicePixels(2)), &[0; 16]).unwrap();
            let first = crate::platform::DRAW_BENCH_TILES.with(|tiles| tiles.borrow()[&(a.texture_id.index, a.tile_id.0)]);
            let mut backend = crate::HeadlessAtlasBackend::default();
            let b = backend.insert(tile.texture_id.kind, crate::size(crate::DevicePixels(2), crate::DevicePixels(2)), &[255; 16]).unwrap();
            let second = crate::platform::DRAW_BENCH_TILES.with(|tiles| tiles.borrow()[&(b.texture_id.index, b.tile_id.0)]);
            assert_eq!(a, b);
            assert_ne!(first, second);
            crate::platform::DRAW_BENCH_TILES.with(|tiles| *tiles.borrow_mut() = saved.clone());
            crate::platform::DRAW_BENCH_TILES.with(|tiles| tiles.borrow_mut().insert(key, first));
            let first_scene = self.draw_benchmark_scene().5;
            crate::platform::DRAW_BENCH_TILES.with(|tiles| tiles.borrow_mut().insert(key, second));
            assert_ne!(first_scene, self.draw_benchmark_scene().5, "atlas mutation escaped digest");
            crate::platform::DRAW_BENCH_TILES.with(|tiles| *tiles.borrow_mut() = saved);
            assert_eq!(before, self.draw_benchmark_scene().5);
            self.rendered_frame.scene.paths[0].color = original_background;
        }
        #[allow(missing_docs)]
        pub fn draw_benchmark_scene(&self) -> (usize, usize, usize, usize, usize, u64) {
            use std::hash::{Hash, Hasher};
            let scene = &self.rendered_frame.scene;
            assert!(scene.surfaces.is_empty(), "surface equality is unsupported");
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            format!("{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}", scene.paths, scene.quads,
                scene.shadows, scene.underlines, scene.monochrome_sprites, scene.subpixel_sprites,
                scene.polychrome_sprites, scene.surfaces).hash(&mut hash);
            for background in scene.paths.iter().map(|p| &p.color).chain(scene.quads.iter().map(|q| &q.background)) {
                format!("{:?}", (background.tag, background.color_space, background.solid,
                    background.gradient_angle_or_pattern_height, background.colors)).hash(&mut hash);
            }
            for tile in scene.monochrome_sprites.iter().map(|s| &s.tile)
                .chain(scene.subpixel_sprites.iter().map(|s| &s.tile))
                .chain(scene.polychrome_sprites.iter().map(|s| &s.tile)) {
                crate::platform::DRAW_BENCH_TILES.with(|tiles| {
                    tiles.borrow().get(&(tile.texture_id.index, tile.tile_id.0))
                        .expect("missing atlas raster digest").hash(&mut hash);
                });
            }
            let glyphs = scene.monochrome_sprites.len() + scene.subpixel_sprites.len();
            (scene.paths.len(), scene.paths.iter().map(|path| path.vertices.len()).sum(),
                scene.paths.len() + scene.quads.len() + scene.shadows.len() + scene.underlines.len()
                + glyphs + scene.polychrome_sprites.len() + scene.surfaces.len(),
                crate::element::DRAW_BENCH_ELEMENTS.with(|count| count.get()), glyphs, hash.finish())
        }
    }
    """)
    test_context = scratch / "src/app/test_context.rs"
    test_context.write_text(test_context.read_text() + """
impl TestAppContext {
    #[allow(missing_docs)]
    pub fn draw_benchmark_scale(&self, window: AnyWindowHandle, scale: f32) {
        self.test_window(window).simulate_scale_factor_change(scale);
    }
}
""")
    app = scratch / "src/app.rs"
    app.write_text(app.read_text() + """
impl App {
    #[allow(missing_docs)]
    pub fn draw_benchmark_assets(&mut self, assets: impl AssetSource) {
        let source = std::sync::Arc::new(assets);
        self.asset_source = source.clone();
        self.svg_renderer = SvgRenderer::new(source);
    }
}
""")
    platform = scratch / "src/platform.rs"
    replace_once(platform, "        _bytes: &[u8],\n    ) -> Result<AtlasTile> {", "        bytes: &[u8],\n    ) -> Result<AtlasTile> {")
    replace_once(platform, "        let tile_id = self.next_id;\n        Ok(AtlasTile {", """        let tile_id = self.next_id;
        use std::hash::{Hash, Hasher};
        let mut digest = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut digest);
        DRAW_BENCH_TILES.with(|tiles| tiles.borrow_mut().insert((texture_id, tile_id), digest.finish()));
        Ok(AtlasTile {""")
    platform.write_text(platform.read_text() + """
thread_local! {
    pub(crate) static DRAW_BENCH_TILES: std::cell::RefCell<std::collections::HashMap<(u32, u32), u64>> = std::cell::RefCell::new(std::collections::HashMap::new());
}
""")
    lib = scratch / "src/gpui.rs"
    lib.write_text(lib.read_text() + """
#[allow(missing_docs)]
pub fn draw_benchmark_observe(enabled: bool) {
    element::DRAW_BENCH_ENABLED.with(|flag| flag.set(enabled));
}
""")
    marker.write_text(json.dumps({"input": identity, "output": tree_digest(scratch)}))
    return identity


def patched_lock(original):
    text = original.decode()
    pattern = r'(\[\[package\]\]\nname = "gpui-pre"\nversion = "0.3.9"\n)source = "[^"\n]+"\nchecksum = "[^"\n]+"\n'
    changed, count = re.subn(pattern, r'\1', text)
    if count != 1:
        raise RuntimeError("expected one registry gpui-pre lock entry; run stock cargo first")
    return changed.encode()


def run_child(command, root, env, capture=False):
    lock = root / "Cargo.lock"
    original = lock.read_bytes()
    expected = patched_lock(original)
    try:
        if capture:
            return subprocess.run(command, cwd=root, env=env, stdout=subprocess.PIPE, text=True, check=True)
        return subprocess.call(command, cwd=root, env=env)
    finally:
        current = lock.read_bytes()
        if current not in (original, expected):
            raise RuntimeError("Cargo.lock changed unexpectedly; preserved current bytes; exclusive Cargo use required")
        if current != original:
            lock.write_bytes(original)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--debug", action="store_true")
    parser.add_argument("--symbols", action="store_true", help="retain test executable symbols for a separate profile")
    parser.add_argument("--samples", type=int, default=31)
    parser.add_argument("--mode", choices=["validate", "clean"], default="validate")
    args = parser.parse_args()
    if not 5 <= args.samples <= 100_000:
        parser.error("--samples must be in 5..=100000")
    for name, maximum in [("PROCESSES", 100_000), ("CORES", 256), ("WIDTH", 16_384), ("HEIGHT", 16_384)]:
        value = os.environ.get("GTOP_DRAW_" + name)
        if value is not None and (not value.isascii() or not value.isdecimal() or not 1 <= int(value) <= maximum):
            parser.error(f"GTOP_DRAW_{name} must be in 1..={maximum}")
    root = Path(__file__).resolve().parents[3]
    directory = root / "target/draw-benchmark"
    directory.mkdir(parents=True, exist_ok=True)
    with (directory / "invocation.lock").open("w") as ownership:
        fcntl.flock(ownership, fcntl.LOCK_EX)
        cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
        matches = list((cargo_home / "registry/src").glob("*/gpui-pre-0.3.9"))
        if len(matches) != 1:
            parser.error(f"Expected one cached pinned gpui-pre-0.3.9; run cargo fetch --locked first; got {matches}")
        scratch = directory / "gpui-pre-0.3.9"
        identity = prepare(matches[0], scratch)
        print(f"GPUI source={matches[0]} instrumentation_sha256={identity} mode={args.mode}", flush=True)
        command = ["cargo", "--config", f'patch.crates-io.gpui-pre.path="{scratch}"', "rustc", "-p", "gtop", "--tests", "--message-format=json"]
        if not args.debug:
            command.append("--release")
        command += ["--", "--cfg", "gtop_draw_benchmark"]
        if args.symbols:
            command += ["-C", "debuginfo=1", "-C", "strip=none"]
        env = dict(os.environ, GTOP_DRAW_N=str(args.samples), GTOP_DRAW_MODE=args.mode)
        built = run_child(command, root, env, capture=True)
        executables = [item["executable"] for line in built.stdout.splitlines() if (item := json.loads(line)).get("executable") and item.get("profile", {}).get("test")]
        if len(executables) != 1:
            raise RuntimeError(f"expected one test executable, got {executables}")
        return subprocess.call([executables[0], "full_monitor_draw_benchmark", "--ignored", "--nocapture", "--test-threads=1"], cwd=root, env=env)


if __name__ == "__main__":
    raise SystemExit(main())
