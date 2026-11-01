# Caveman FX DLL: audit (2026-10-21, evening)

An audit of `crates/caveman-fx` (the DLL injected into skate.: the Present hook, the depth occlusion, the transform proxies, The Caveman's texture writer) and of the two crashes of 2026-10-21 18:29 and 18:49. Written after reading every module of the DLL, the trainer's inject path, the host tests, the game's crash dumps and logs, and the Direct3D 12 documentation on the points that matter (sources at the end). Trevor asked for this after the second crash: "we're doing some dangerous stuff messing with GPU buffers, any way to add some guardrails?"

## Verdicts, shortest form

- **The two crashes were the game's own DirectX out-of-memory failure, and the memory spike that causes it happens with no DLL loaded and with a stock deck.** Both crash dumps hold the same exception (`0x8007000e`, E_OUTOFMEMORY, raised by the game's fatal handler after its dialog). The game's log shows a transient spike at every loading-screen skater rebuild (a fast travel, the first spawn): process commit from about 14 GB to 24 to 29 GB, GPU system memory from about 0.5 GB to 8.5 to 12 GB, video memory up 1 to 2 GB, gone within ten seconds. The spike appears before the DLL was injected in both crashed sessions, in the session started at 18:56 with nothing injected, and with Natural Blank equipped (the A/B test in section 1). Whichever allocation lands on the peak loses; twice it was the game's upload heap, once DWM and the Snipping Tool went with it. The Caveman and the DLL are both cleared of causing the spike, and so are the cosmetic mods (parking 54 of them changed nothing); the burst is the game streaming the world in at a new location, and the hazard is that burst against a card that other apps already fill by 4.6 GB.
- **Round 4 of the DLL (r5) did make the first crash likelier, and that part is fixed in the working tree.** It remade its four 4K depth copies (133 MB a set) at every size change of the main depth and kept replaced sets for three seconds; a pause flapped the main depth three times in a second. The working tree now makes one set and keeps it, follows a new main depth only after 24 steady windows, and frees a replaced set on a fence. Two of the new pieces need changing before they ship (below: C1, C2).
- **The hooking design is sound and the memory reads are safe.** Vtable slot swaps only, no code patched, every call chained; all game memory read through `ReadProcessMemory` on the own process, so an unmapped page is a failed read, never a fault; panics caught at every hook boundary; the dangerous work (recording into the game's command lists) is gated on a learnt state and skipped on any mismatch. The remaining risks are lifetime and state correctness on the GPU side, which the debug layer cannot see in the game, so they are listed with host tests that can.
- **The Caveman's texture write reports success but the board stays white.** The write path is right by construction (footprints from `GetCopyableFootprints`, verified byte for byte in the host test), so the open question is whether the game samples that resource at all. A read-back after the write settles half of it in one run.

## 1. The crashes

### What the dumps and logs say

- `E:\Games\ReSkate\logs\crashes\fd7bd1...\sent\106992-...dmp` (18:29) and `114056-...dmp` (18:50): both `exception 0x8007000e at KERNELBASE.dll+0xc41ca` (RaiseException), `capture.source: game-fatal-handler`, the stack through `D3D12Core.dll` and `nvwgf2umx.dll` under `Skate.exe+0x4001031`. The dialog Trevor photographed names `RenderCore2\...\Dx12\ResourceUploadHeapDx12.cpp`: `CreateCommittedResource` of an upload heap in `GENERIC_READ`.
- Windows Application log: 18:29:36 the Snipping Tool crashed with `8007000e` (E_OUTOFMEMORY) while the game's dialog was up; 18:49:44 `dwm.exe` crashed in `dwmcore.dll` with `0xc00001ad`, six seconds after the game's dialog appeared (the game log shows the dialog window class `#32770` taking focus at 18:49:38). No driver reset (no event 4101) either time.
- The game's memory lines (`SDK: [Runtime] [INFO] Memory: ... GPU: X video memory, Y system memory`), in order:

| Session | Time | Event | Committed | GPU video | GPU system |
|---|---|---|---|---|---|
| 1 (DLL in at 18:28:15) | 18:27:20 | level loaded | 11.6 GB | 4.55 GB | 0.43 GB |
| 1 | 18:27:30 | first skater creation | 19.8 GB | 7.82 GB | 4.91 GB |
| 1 | 18:27:40 | | 14.0 GB | 6.80 GB | 0.46 GB |
| 1 | 18:29:17 | skater creation 4, then the dialog | 22.9 GB | 7.30 GB | 8.53 GB |
| 2 (DLL in at 18:41:22, depth off) | 18:37:46 | riding | 14.1 GB | 6.82 GB | 0.48 GB |
| 2 | 18:39:36 | item browser (before injection) | 24.0 GB | 8.56 GB | 8.53 GB |
| 2 | 18:39:46 | | 14.2 GB | 6.84 GB | 0.48 GB |
| 2 | 18:45:56 | skater creation 8 | 24.4 GB | 8.62 GB | 8.52 GB |
| 2 | 18:46:06 | | 14.7 GB | 6.92 GB | 0.49 GB |
| 2 | 18:48:05 to 18:48:53 | skater creations 9, 10, 11 | | | |
| 2 | 18:49:38 | the dialog | | | |
| 3 (nothing injected) | 18:57:18 | level loaded | 11.6 GB | 4.55 GB | 0.42 GB |
| 3 | 18:57:30 | first skater creation | 22.1 GB | 6.89 GB | 8.48 GB |
| 3 | 18:57:40 | | 14.0 GB | 6.70 GB | 0.45 GB |

- Other processes held 4.6 GB of the 16.4 GB of VRAM during session 3 (DWM 1.7 GB at 4K HDR, steamwebhelper 0.86 GB, Firefox 0.7 GB, Wallpaper Engine 0.5 GB, NVIDIA Broadcast 0.24 GB), the game 7.1 GB at rest. The spike's extra 1.7 GB of video memory and 8 GB of system-backed GPU memory land on top of that.

### What the documentation adds

- An UPLOAD heap lives in system memory (D3D12 memory pool L0, DXGI's NON_LOCAL segment on a discrete card); a DEFAULT heap texture lives in video memory (L1, LOCAL). Budgets are tracked per process and per segment, and "the creation APIs will return failure" when a process is over budget; the kernel moves heaps from video to system memory only "as an extreme last resort" (Microsoft, residency). So video memory pressure from one allocator can surface as a failed system memory allocation elsewhere, through demotion, but the documents do not describe that chain. The dumps do not record the budgets at the moment of failure; the DLL's next build logs both segments every second so the next failure can be attributed.

### The A/B test (19:00 to 19:10, nothing injected)

Trevor respawned with three decks while the memory monitor ran (`scratchpad\gpumon.sh` writes `gpumon\samples.txt` and keeps a copy of each session's `ReSkate.log`, which the game overwrites at every launch). Two kinds of respawn appear in the log: a `Skater creation` (a session marker respawn) and a `Skater component factory` rebuild (a loading screen: fast travel from the map).

| Deck | Marker respawns | Loading-screen rebuilds | Spikes |
|---|---|---|---|
| Natural Blank (stock) | 4, 5, 6 | none | none |
| 000 - FX Combo (7 effects) | 7 to 10 | none | none |
| 000 - The Caveman (11 effects) | 11 to 19 | factories 12, 14, 16, 18 | 19:04:00 (25.7 GB, 8.5 GB system), 19:05:00 (25.1 GB, 9.3 GB) |
| Natural Blank again (the control) | 20 to 24 | factories 20 to 28 | 19:09:30 (24.9 GB, 8.55 GB), 19:09:40 (29.0 GB, 12.25 GB system, 9.0 GB video), 19:10:01 (24.9 GB, 8.6 GB) |

**Result: the spike is the loading-screen rebuild, with any deck.** Marker respawns never spike; every rebuild does, stock deck included, and the biggest spike of the day came with Natural Blank. The Caveman is cleared, and so is the DLL. The two crashes were rebuild spikes landing on a card that was already within about 2.5 GB of full (the game at 9 GB at the peak, DWM 1.7 GB for 4K HDR, other apps 2.9 GB). Trevor had noticed the rebuild lag before today.

### The second test: the cosmetic mods parked (19:25 to 19:30)

The first guess at the bulk was the merged customization bundle (every mod item in `cas_main_sharedbundle`, mod textures written whole). To measure it, 54 cosmetic mods (every deck pack, the clothing packs, griptape and wheels, 3.4 GB) were moved to `E:\Games\ReSkate\Mods-parked` and the game relaunched with 8 item mods merged instead of 60.

| Event | Committed | GPU video | GPU system |
|---|---|---|---|
| level loaded | | | |
| first spawn (factory 1 and 2) | 23.7 GB | 8.40 GB | 8.65 GB |
| ten seconds later | 13.9 GB | 6.66 GB | 0.46 GB |
| fast travel (factory 4) | 22.6 GB | 9.67 GB | 5.53 GB |
| ten seconds later | 14.0 GB | 6.69 GB | 0.47 GB |

**The spike is unchanged, so the cosmetic mods are not it.** What a rebuild and a marker respawn do not share is the location: a first spawn and a fast travel stream a whole area of the world in through upload heaps, a marker respawn keeps the location and streams nothing. The burst is the game's own world streaming (with seven maps installed and 4K textures), and the crash risk is the burst against the headroom left on the card.

### What to do

1. Headroom first: closing Wallpaper Engine, Firefox, Steam's web views and NVIDIA Broadcast before a session returns 2 to 3 GB of VRAM, more than the spike's video part. DWM's 1.7 GB is the price of 4K HDR.
2. The parked mods may stay parked (a faster merge and a smaller inventory; every deck of theirs is in `Caveman Imports`) or come back; it makes no difference to the spike.
3. The DLL's own guardrails (section 6) stay: the game runs close enough to the edge that nothing injected may allocate casually, and the next failure must be attributable (both segments logged every second).

## 2. The depth occlusion (`depth.rs`, the copy half of `overlay.rs`)

How it works, in one paragraph: the game's command lists get their own function tables from D3D12Core (one per list, inside the list object, with the driver's own functions in about forty slots: the Runtime Bypass feature of the Agility SDK). The DLL patches five slots of each list's table the first time the list is submitted (`ResourceBarrier`, `RSSetViewports`, `Barrier`, `OMSetRenderTargets`, `Reset`), calling through to what the slot held. When the game's own barrier takes the main depth out of `DEPTH_WRITE` into the learnt G-buffer state (0xc0), the hook records, in the same list, a transition of plane 0 to `COPY_SOURCE`, a `CopyTextureRegion` into one of four R32_FLOAT textures of ours, and the transition back. The `ExecuteCommandLists` hook notes which copies were submitted and in which present window. At Present the overlay draws the bars with the copy for the picture being shown, testing each pixel in the pixel shader.

### Findings

**C1 (critical, working tree). Copies into a destination larger than the source are not confirmed legal.** The OOM fix makes one set at the larger of the back buffer's size and the depth's and copies any smaller depth "into its top-left corner". Microsoft's rule for `CopyTextureRegion` with a depth-stencil source: "you must copy the entire subresource rectangle ... pass 0 to the DstX, DstY, and DstZ parameters and NULL to the pSrcBox parameter", with identical sample counts. The call as written satisfies that, but no document says a larger destination is accepted, and the release host test fails before it reaches its debug-layer report, so it has not been checked. Fix: keep the set at the main depth's exact size; when the chosen depth does not match the set, take no copy (the test goes late, then off, with the existing hysteresis) rather than copy into a mismatched texture; remake only after the new size has been steady for `SWAP_STREAK` windows, at most once in five seconds, under the budget check. If a host test with the debug layer proves the larger destination clean for both `R32_TYPELESS` and `D32_FLOAT_S8X24_UINT` sources, the top-left-corner scheme may come back; until then exact size only.

**C2 (critical, working tree). A recorded copy dropped by age can still reach the GPU after its textures are freed.** `inject` keeps a `Pending` per recorded copy and drops entries older than eight presents; `set_done` then treats the set as unused and the overlay releases it. A list recorded and submitted later than eight presents (the game has no obligation to submit promptly) would then read freed textures: a GPU page fault and device removal. Fix: drop a list's pending copies in the `Reset` hook (a reset discards the recording, so nothing can submit it), never by age; a pending copy older than 60 seconds is logged and the set is kept alive (leaking one set is better than a fault). Microsoft: "Before releasing the final reference on the resource, your application must ensure that the GPU will no longer read nor write to this resource", and only the debug layer reports a violation.

**C3 (critical, r5 and working tree). A copy on one queue read by a draw on another, with no wait.** The copy list goes wherever the game submits it; the overlay draws on the presenting queue. On the same queue the order is guaranteed ("Calling ExecuteCommandLists twice in succession ... guarantees that the first workload finishes before the second"); across queues it is not, and "a fence must be used to synchronize after writing before another queue can read the written bytes". The working tree signals `SYNC_FENCE` on the copy's queue but never publishes the fence, and the draw never waits on it. Fix: the draw queue waits on the fence value recorded with the copy (`Latest.fence`) when `Latest.queue` is not the draw queue, or, simpler and preferred, copies submitted on another queue than the presenting one are marked unusable and counted. One fence per queue ("It is recommended that each fence only be used to indicate progress on one timeline"): a fence signalled from two queues can go backwards.

**H1 (high). The copy format pair is not documented.** The game's main depth is `D32_FLOAT_S8X24_UINT`; the copy takes its plane 0 (subresource 0) into `R32_FLOAT`. The planar depth spec places that plane in the `R32_TYPELESS` family for staging copies, and `CopyTextureRegion` requires formats "identical or from the same type group", but no document states that a texture-to-texture copy from the depth plane of an R32G8X24 texture into an R32 texture is allowed. It works on this driver (the deck calibration reads back the game's near plane of 0.06 m exactly, so the data is right), and the host test covers only `R32_TYPELESS` with a `D32_FLOAT` view. Fix: a host test variant with a `R32G8X24_TYPELESS` depth and a `D32_FLOAT_S8X24_UINT` view, under the debug layer. If the layer objects, the documented-safe destination is `R32G8X24_TYPELESS` viewed as `R32_FLOAT_X8X24_TYPELESS` (same type group), at 8 bytes a texel, with the ring cut to two.

**H2 (high). Barrier state is the one thing a wrong guess turns into device removal.** The copy's transitions start from the state the game's barrier just set, so they are right by construction; the "first pass end of the window" and "G-buffer state learnt over 64 windows" rules keep them away from other passes. Microsoft on `ExecuteCommandLists`: if a transition's before state "does not match up with the after state of a previous transition, then the runtime will drop the call and remove the device". The current code is fine here; the finding is that no new copy site should ever use an inferred state, and that `after` states with a write bit (RENDER_TARGET, UNORDERED_ACCESS, COPY_DEST) should be excluded from copying even if the signature learns one (a depth left in a write state is being written by the next pass).

**M1 (medium). Ring slots can be reused under a history entry.** `LATEST` keeps the last four submitted copies and `PENDING` can hold more; a slot recorded again invalidates the older entry on it, but the history still lists it. Harmless today (the late path then reads newer data, which is the better copy), but make it explicit: recording into slot s drops history entries on s.

**M2 (medium). The alignment test cannot decide.** Its probes sit at the deck's centre, which is mostly the skater's shoes; the three offsets scored 55, 54 and 53 percent. Replace it with exact pairing: when a copy is recorded, snapshot the camera and board proxies' frame matrices (`+0x50`) and previous matrices (`+0x90`) into `Pending`; at Present, the copy whose snapshot equals the camera the bars are drawn with (byte-equal `+0x50`, or the frame's `+0x90` equal to the copy's `+0x50` for a one-frame lag) is the frame's own, with no counting or statistics. The `diag` record should carry the match kind per present.

**M3 (medium). With the depth test off, the shared-table hooks are never taken out.** `adopt` returns early when disabled, so every list made afterwards copies our hooks from the template (they pass through, so nothing breaks), and only the unload-time memory scan puts them back. Fix: release the template hooks when the test is turned off, or install the command list hooks only when the test is turned on.

**M4 (medium). A game thread can wait on a lock held by a status request.** `note_end` takes `CANDIDATES` blocking on the recording thread; `status()` holds it while building JSON. Keep `status()` on `try_lock` or copy the list out quickly.

**L1 (low). Copies on a `COMPUTE` list are refused, bundles too** (`GetType() != DIRECT`): right. **L2 (low).** `describe` calls `GetDesc` on every pass end: a COM call per barrier, cheap, and the resource is alive because the barrier references it. **L3 (low).** DLSS Frame Generation presents frames under the DXGI hook, so the bars would appear on alternate frames; Frame Generation is unsupported and should be said so in the docs.

### A simpler design worth a prototype: copy at Present

The game's main depth is a persistent resource (one address for 533,000 presents), and the game's own last transitions in every window leave it in `COPY_SOURCE` (`pass_ends_by_window: ... 0x800 later x2`). So at Present the depth of the picture being shown is sitting in `COPY_SOURCE` with the frame complete, which is what ReShade's generic depth add-on relies on. Our own command list, executed on the presenting queue before the bars' draw, could copy it then: no recording into the game's lists, no per-picture bookkeeping, no cross-queue question, and the copy is the frame's own by construction, which removes the whole class of "which window was this copy in" blinks. What it needs: the state of the depth as of the last *submitted* list, which means replaying each list's observed transitions at `ExecuteCommandLists` into a per-resource state table (the barrier hooks already see the transitions; today they act at record time). When the replayed state is not a read state (a next frame's list already submitted), skip the copy for that present. Trade-offs: the depth at Present includes everything that wrote depth after the G-buffer (decals, transparents with depth write, rarely a UI pass), which for occlusion is as good or better; the copy costs one more `ExecuteCommandLists` a frame on the presenting queue. Recommended as a `draw depth mode present|pass` toggle, default `pass` until the blink counters say which is better.

## 3. The overlay's own GPU work (`overlay.rs`, draw half)

- Allocations happen in two places only: `Gpu::new` (once per device: four 512 KB upload vertex buffers, a read-back buffer, the descriptor heaps, the counters) and `ensure_copies`. Nothing is created per frame. `board.rs` makes its objects once and grows its upload buffer only when a larger chain arrives (never, at one size). This was checked line by line after the crash; the DLL holds about 4 MB with the depth test off.
- The draw transitions the back buffer `PRESENT -> RENDER_TARGET -> PRESENT` on a flip-model swap chain under Streamline, obtains the buffer with `GetBuffer` each frame and releases it in the frame, so a `ResizeBuffers` never finds our reference. Right.
- `draw` waits at most 2 ms for the allocator ring and skips the frame otherwise; a device removal (`GetCompletedValue == u64::MAX`) ends drawing. Right.
- `note_change`, `hold_back` and `ensure_copies` log from the render thread (`worker::log` takes the worker's mutex and appends to a file): a hitch at each depth-test change. Queue the line for the worker instead, as `board.rs` does with `say_later`.
- `diag::window_end` runs on the game's recording threads and takes `RECORD` blocking; `diag::write` serialises 2048 entries to JSON and writes the file under that lock. A game thread would wait inside `ResourceBarrier` for the whole write. Fix before wiring: `try_lock` in `window_end` (a dropped sample is nothing), and copy the entries out, then serialise and write with the lock released.
- Blending in HDR10 happens in PQ space; the solid style replaces exactly, the additive glow is approximate. Cosmetic.

## 4. The Caveman's texture writer (`board.rs`)

- The chain check is thorough (vtables, type descriptor, name, format, size, mips, the owner hash, `GetDesc` and the device on the render thread); only the texture found this way is ever written. The upload uses `GetCopyableFootprints`, row by row, twelve `CopyTextureRegion`s, transitions to `COPY_DEST` and back to exactly the state decided. The host test reads every mip back byte for byte on a placed resource in 0xc0, with the debug layer clean, on both the system D3D12 and the game's Agility SDK.
- The state is inferred (0xc0 from RenderCore2's bind flags 8 and 8) unless the game's own barriers were seen on the resource. A wrong before state is the documented device-removal case (H2). The write at 18:28:40 did not remove the device, which says the state was right or the runtime does not validate; neither is proof. Rule: inferred states stay allowed for The Caveman only, and only while the probe (`board probe`) agrees; any other texture needs an observed state.
- Why the board stayed white is open. The write landed on the GPU (the fence passed), so the candidates are: the deck is drawn from another resource (a second resident copy, a composited texture built at equip time, a streaming copy), or the game re-uploaded the original mips from its cache after our write. Three checks, all read-only or one write each: (1) `board verify`: copy a 4 x 4 block of mips 0 and 2 back into a read-back buffer right after the upload and again two seconds later, and compare with what was sent; (2) after a write, re-equip The Caveman from the menu and hover it (a composite rebuilt at equip time would show the art then); (3) with skwatch, count the D3D12 resource objects whose description is 512 x 2048, 12 mips, BC7 sRGB, against the number of such decks loaded (two per deck means a second copy). The research found nothing public about Frostbite's texture residency or customisation compositing, so the game has to be asked.
- `board deck "Dan Da Dan.png"` does not match a file name with its extension (the full path does). Small, fix.

## 5. Hooks, threads, unload

- The vtable slot swaps (`vhook.rs`, `hook.rs`) are 8-byte atomic writes with `VirtualProtect` around them, refused when the slot does not hold what is expected, and chained: ReSkate's MinHook detours on the same functions and the Steam overlay keep working (seen in game all day). Right.
- The per-list table patching (`adopt`) writes with compare-and-swap only into committed read-write memory inside the list object, keeps every patched table for restore, and restores by compare-and-swap. The driver re-patching a slot later would silently drop our hook, never corrupt the table. The Runtime Bypass spec confirms the mechanism (a per-object table with the driver's entry points for high-frequency calls); `ResourceBarrier` and `CopyTextureRegion` are not among the bypassed calls, which matches what the DLL observed.
- Every hook is wrapped in `catch_unwind` and counted in `INFLIGHT`; uninstall waits for zero. Right. But `unload` is inherently racy: a thread that loaded our pointer from a table just before the restore and calls it after `FreeLibraryAndExitThread` crashes the game; the one-second sleep narrows it, nothing closes it. Rule: `unload` is a development command; for a session that matters, close the game instead.
- The worker's memory scans read only resident pages (`QueryWorkingSetEx`) through `ReadProcessMemory`, 32 MB per tick, so a 6.5 GB scan costs a few seconds of one core and never pages anything in. Right.
- `DllMain`'s detach path only swaps slots back, nothing that could take the loader lock. Right.

## 6. Guardrails

What the next build adds, so a mistake shows up as a log line and a refused allocation rather than a dialog:

1. **A memory watch in the DLL.** `QueryVideoMemoryInfo` for both segments once a second; usage and budget in `draw status`; a log line whenever either moves by more than 256 MB within a second (the game's spikes will show with their time); the DLL refuses any allocation of its own within 384 MB of either budget.
2. **No allocation outside two places.** `Gpu::new` at install and `ensure_copies`, once per depth size, never on size churn, never on a timer; a unit check of the source greps for `CreateCommittedResource`, `CreateHeap`, `CreatePlacedResource` and `CreateReservedResource` and fails if one appears elsewhere.
3. **Fence-gated frees only**, with a test that keeps a list unsubmitted across a set replacement and asserts the set survives.
4. **The memory monitor outside the game** (`gpumon.sh` today, a `caveman-trainer watch` command next): the game's memory line and VRAM every ten seconds, a warning under 1.5 GB free, and a copy of each session's `ReSkate.log`.
5. **A soak in the host test**: 20,000 frames with a flapping depth size, `ID3D12DebugDevice::ReportLiveDeviceObjects` counted at the start and the end, the debug layer's message count zero.
6. **Mods tested like code.** A new FX item gets the respawn-and-watch test (three respawns with the monitor running) before it goes into a session with friends.
7. **No `unload` in sessions that matter; close the game.**

## 7. Host tests to add (all under the D3D12 debug layer, on the game's Agility SDK)

- The depth copy with a `D32_FLOAT_S8X24_UINT` source (H1); with an exact-size destination; with a larger destination (C1), expecting either a clean report or a documented refusal.
- Size flapping: the main depth changes size every third present for 100 presents; one set alive, held bytes flat, no copies taken while mismatched.
- A list recorded with a copy, held unsubmitted while the set is replaced, submitted 20 presents later: the set is still alive, the copy executes, no debug error (C2); the same list reset instead of submitted: the pending entry is gone and the set is freed.
- A copy list submitted on a second queue: the draw waits or skips, never reads early (C3).
- The format-change path: a `D16_UNORM` depth after an `R32` one remakes the set once, under the five-second rule.
- Unload with copies in flight on the host's queue: no debug error, no live objects left.
- The board read-back (`board verify`) on the host's texture, before and after a simulated re-upload by the host.

## 8. What the research established

- Depth-stencil copies: whole subresource only, `pSrcBox` NULL, destination offsets 0, identical sample counts ([CopyTextureRegion](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nf-d3d12-id3d12graphicscommandlist-copytextureregion)); planes are separate subresources and transition separately ([Subresources](https://learn.microsoft.com/en-us/windows/win32/direct3d12/subresources), [Planar depth-stencil spec](https://microsoft.github.io/DirectX-Specs/d3d/PlanarDepthStencilDDISpec.html)).
- Depth-stencil resources are never implicitly promoted; explicitly transitioned textures do not decay to COMMON ([Using resource barriers](https://learn.microsoft.com/en-us/windows/win32/direct3d12/using-resource-barriers-to-synchronize-resource-states-in-direct3d-12)); a mismatched before state at `ExecuteCommandLists` removes the device ([ExecuteCommandLists](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nf-d3d12-id3d12commandqueue-executecommandlists)).
- Release only after the GPU is done; the fence pattern ([Fence-based resource management](https://learn.microsoft.com/en-us/windows/win32/direct3d12/fence-based-resource-management), [CreateCommittedResource](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nf-d3d12-id3d12device-createcommittedresource)).
- Budgets per process and segment, failure when over budget, demotion as a last resort ([Residency](https://learn.microsoft.com/en-us/windows/win32/direct3d12/residency), [QueryVideoMemoryInfo](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_4/nf-dxgi1_4-idxgiadapter3-queryvideomemoryinfo), [D3D12_MEMORY_POOL](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/ne-d3d12-d3d12_memory_pool)).
- Cross-queue reads need a fence; one fence per timeline ([User-mode heap synchronization](https://learn.microsoft.com/en-us/windows/win32/direct3d12/user-mode-heap-synchronization)).
- The per-list function tables are the D3D12 Runtime Bypass ([spec](https://microsoft.github.io/DirectX-Specs/d3d/D3D12RuntimeBypass.html), [Agility SDK blog](https://devblogs.microsoft.com/directx/agilitysdk-1-716-0-preview-and-1-615-retail/)).
- Streamline: third parties should not use SL proxies; a special GUID returns the native interface, which is what the DLL hooks ([Streamline programming guide](https://github.com/NVIDIAGameWorks/Streamline/blob/main/docs/ProgrammingGuide.md)).
- Nothing public on Frostbite's texture residency or on cosmetic compositing in EA's sports titles; the ReSkate repository's `Engine/Game` and `Engine/Resource` are the next place to look.
- `dwm.exe` crashes with `0xc00001ad` in `dwmcore.dll` are reported with fullscreen games and GPU driver trouble, no official cause ([Microsoft Q&A](https://learn.microsoft.com/en-us/answers/questions/3896620/dwm-exe-application-error)).

## 9. The live board session (20:00 to 21:50): what was learnt

Builds r5b to r5e were injected one after another into the same game (bars off, depth test off, no depth copies ever made), each adding one probe, with Trevor reporting the ridden board after every write. Facts established:

- **The write lands.** The read-back check (r5b on) shows every written texture holding the bytes sent, now and two seconds later. The menu's item preview and the Skaters screen's character preview show the written art at once: both are built from the source texture when they open.
- **The ridden board holds a copy made at equip.** Writes to the source never change it; un-equipping and re-equipping does, because the DLL re-writes the re-created source before the game builds the board from it. That is the working path today: write, then re-equip.
- **Not the sampled texture:** the four "siblings" next to the source in the material table (other decks' graphics); the two nameless RGBA8 10-mip textures of the deck's shape (one is a dynamic layer the game rewrites within two seconds and at every equip through compute passes, the other refuses writes; a red fill into them turned the board white, so they are layers the deck shader reads, not the graphic); the two BC7 textures with fixed name hashes `0xb01df88f6992cc49` and `0xd3c3d3936770c86` created together at every equip (written cyan, verified, no change; written before a re-equip they broke the equip and the board came up white).
- **The equip writes, from the upload table (r5d):** two RGBA8 UAV working textures (compute per mip), one RGBA8 sRGB 10-mip result and one BC7 UNORM 12-mip result, both filled by a copy and left in shader-resource state. **Both results are transient:** the RGBA8 one was released within minutes (its COM object's memory reused), and writing into the BC7 one after its release took the device down (the third crash of the day, 21:49, mine). The copy from those results into the ridden board's persistent texture was never seen by the barrier hooks, so it happens on a copy queue or without a barrier.
- **The later rounds (r5f to r5i, to 00:03):** the copy tracer showed the world's textures are ordinary mip-streamed 2D textures filled from staging buffers on the copy queue (the 256 x 256 arrays were a wrong turn); the RGBA8 "compositor" is the game's general transcode scratch (copies out of it go to textures of every shape, and writes into it never persist); a 1024 x 2048 BC7 texture written at equip is a UI texture (its magenta flashed at the top-left of the screen); copies from the asset texture are never seen by the hooks, so the ridden copy is built by shaders. The re-equip race was measured: the DLL re-finds the new asset texture 6 to 11 s after the loss, and the game rebuilds the ridden board before that, so the board shows the kept art only when the timing happens to favour it. A colour hunt that reads every known texture under an assumed state found released resources and removed the device (the fourth crash, 00:03): without a liveness signal, GPU-wide reads are as unsafe as writes.
- **The last test (r5j, 00:35):** with the hooks in before a deck change, equipping the solid red Colour Test deck made the game upload one deck-shaped texture, a 512 x 2048 BC1 with 12 mips whose owner carries a fixed name hash (`0x0cb4cea5efea33dd`, session-persistent, re-filled at every equip). A BC1 chain was added, the texture found by its hash and written magenta (read-back match), and the ridden red board did not change: the last candidate the hooks can see is not the graphic either (probably a board part that shares the deck's UV layout). Every texture written at an equip that the hooks can observe has now been tested.
- **Resolved (2026-10-22 morning, r5k to r5m; details in `fx-hook-plan.md`).** RenderDoc was a dead end here: the launcher refuses a process whose `LoadLibraryW` is not a KnownDLL's, and Streamline's interposer resolves the D3D12 entry points by walking the export table itself, so even injected before ReSkate.dll RenderDoc never wrapped the device. The liveness signal came from `ID3DDestructionNotifier` (no hook: a destruction callback per described resource, all undone at unload), the hunt learnt to read the whole census of runtime texture objects with a reference held per batch, and a stall was fixed (a COPY_SOURCE to COPY_SOURCE barrier made the batch's list refuse to close). The needle method then found it in one pass: **the ridden board samples a nameless 256 x 1024 R8G8B8A8_UNORM_SRGB 9-mip plain shader resource, the deck graphic demoted by the streamer to half size**; writing it changes the board at once, no re-equip. The DLL now finds it at every equip by rule (`ridden`) and writes the kept art into it fitted to its size. Every candidate listed above was indeed not it; the menu previews read the asset, the world reads this copy, and nothing the barrier hooks saw ever touched it because it is filled by the streamer's own path.

## 10. Open questions for Trevor

- Was "000 - The Caveman" equipped in the 18:34 session (the outfit is saved in the profile, so it stays equipped across launches unless changed)?
- What happened at the crash moments: a respawn, a friend joining (participants joined at 18:44 and 18:48), the menu?
- Do the respawn spikes show with a stock deck (the A/B test)?
