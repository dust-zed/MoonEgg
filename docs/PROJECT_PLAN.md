# MoonEgg 项目计划

状态：Draft  
更新时间：2026-09-06  
规划周期：约 24 周，按每周 8–12 小时设计

## 1. 项目使命

MoonEgg 不是为了快速拼出一个播放器 App，而是用一个真实、可演示、可维护的工程，系统掌握以下能力：

1. Rust 所有权、错误处理、并发、FFI 和 unsafe 边界。
2. 媒体容器、编解码、采样、像素、时间戳、缓冲和音画同步。
3. Android NDK、JNI、AAudio、MediaCodec、Surface 和应用生命周期。
4. 工程设计、测试、性能分析、文档、版本发布和开源协作。

最终求职材料不只是“会 Rust”或“做过播放器”，而是一条完整证据链：设计文档、可复现实验、测试、性能数据、问题复盘、演示应用和稳定版本。

## 2. 产品边界

### 2.1 第一个完整版本（v0.1）

在 Android API 26 及以上真机运行，支持：

- 从文件描述符打开本地 MP4。
- H.264 视频和 AAC 音频。
- prepare、play、pause、seek、stop、release。
- 视频输出到 `Surface`，音频输出到系统音频设备。
- 以音频时钟为主时钟的基础音画同步。
- Activity 前后台切换、Surface 重建、耳机切换等基本生命周期场景。
- 结构化错误和播放指标：首帧时间、卡顿次数、丢帧数、队列水位、当前位置。
- 一个最小 Kotlin Demo 和可重复运行的测试媒体集合。

### 2.2 非目标

v0.1 不支持网络流、直播、DRM、字幕、倍速、投屏、播放列表、后台服务和多平台 UI；不自研 H.264/AAC 解码器。控制范围比功能数量重要。

### 2.3 技术取舍

- **Android 优先，但核心不依赖 Android 类型。** 平台能力通过 trait/adapter 接入。
- **音频优先。** 音频链路更适合先学习实时回调、缓冲和时钟。
- **同步线程优先。** 播放管线是长生命周期、背压明确的工作负载，第一版使用专用线程和有界队列，不先引入大型 async runtime。
- **Kotlin/Rust 边界要窄。** JNI 只传命令、句柄、Surface、文件描述符和少量事件，不跨边界搬运媒体帧。
- **unsafe 集中管理。** 每个 unsafe 块说明不变量；原始指针不得泄漏到安全核心。
- **先借助平台编解码，再建设可替换后端。** v0.1 用 Android NDK 的媒体能力完成闭环；后续再评估 FFmpeg 软件后端，不让第三方构建系统阻塞最早的学习反馈。

Android 官方建议尽量缩小 JNI 层并减少跨边界的数据搬运；AAudio 是 API 26 起支持的原生低延迟音频 API。这两点决定了 v0.1 的最低版本和 FFI 边界：

- [Android JNI tips](https://developer.android.com/ndk/guides/jni-tips)
- [Android AAudio guide](https://developer.android.com/ndk/guides/audio/aaudio/aaudio)
- [Android NDK media APIs](https://developer.android.com/ndk/reference/group/media)
- [Android Native Window](https://developer.android.com/ndk/reference/group/a-native-window)

## 3. 建议架构

```text
Kotlin Demo / public API
          │ commands + events
          ▼
    thin JNI / C ABI
          │ opaque player handle
          ▼
┌──────────────────────────────────────────┐
│ Rust player core                         │
│ state machine · clock · scheduler        │
│ command loop · metrics · error model     │
└─────────────┬────────────────────────────┘
              │ platform-neutral traits
     ┌────────┴─────────┐
     ▼                  ▼
demux/decode        audio/video sinks
Android NDK         AAudio / Surface
     │                  │
     └── bounded packet/frame queues ──────┘
```

建议的工作区在需要时逐步长成以下结构，不提前创建空 crate：

```text
crates/
  moonegg-core/       # 状态机、命令、事件、时钟、调度；可在主机测试
  moonegg-media/      # 媒体数据模型和 demux/decoder trait
  moonegg-android/    # NDK 后端与 Android 专属资源封装
  moonegg-ffi/        # JNI/C ABI、句柄表、panic/error 转换
apps/android-demo/    # Kotlin 示例应用
fixtures/             # 小型、来源和许可证明确的测试媒体
docs/adr/             # Architecture Decision Records
```

### 3.1 核心约束

- 控制线程拥有播放器状态，其他线程通过命令和事件通信。
- packet/frame 队列必须有界，满时产生背压，不能无限增长。
- 音频实时回调内不分配内存、不阻塞、不加互斥锁、不写日志。
- 播放器句柄具有明确所有者，`release` 幂等；释放后调用返回结构化错误。
- JNI 入口捕获 Rust panic，任何 panic 都不能跨 FFI 边界。
- `Surface`、codec、audio stream 的创建和销毁遵循各自线程约束。
- 时间统一为带单位的新类型，不用裸 `i64` 混用微秒、毫秒和帧数。

### 3.2 状态机

首版状态控制在：

```text
Idle → Preparing → Ready → Playing ↔ Paused
  ↑         │         │       │          │
  └─────────┴─────────┴───────┴──────────┘
                    stop/release

任意活动状态 ── fatal error ──→ Error
```

每个公开操作都要定义允许状态、状态迁移、是否同步、失败语义和幂等性。状态机先以纯 Rust 单元测试验证，再接 Android。

## 4. 24 周路线图

时间是建议节奏，不是截止日期。每个阶段只有通过退出条件才进入下一阶段。

### M0：工程地基（第 1–2 周）

**交付**

- Rust workspace、Android Demo 骨架和一条本地构建命令。
- CI：格式检查、Clippy、单元测试、文档构建。
- `CONTRIBUTING.md`、许可证、Issue/PR 模板、提交约定。
- ADR-0001：范围与架构；ADR-0002：JNI 接口原则。

**学习**

- Cargo workspace、crate 类型、feature、交叉编译。
- Rust 错误模型、模块可见性、测试和文档测试。
- Android ABI、NDK 与 `.so` 打包过程。

**退出条件**

- 新机器按文档可构建并安装 Demo。
- CI 对主机端 Rust 代码全绿。
- Kotlin 调用 Rust `version()`，异常不会导致进程崩溃。

### M1：从字节到声音（第 3–5 周）

**交付**

- 自己实现一个最小 WAV/PCM 解析器，只支持明确的 PCM 子集。
- 主机端命令行工具读取 WAV，输出格式、时长和前若干采样。
- 环形缓冲区与生产者/消费者实验。
- 纯 Rust 测试：损坏头、截断输入、奇数 chunk、边界时长。

**学习**

- RIFF/WAV、端序、采样率、声道、位深、帧和时长换算。
- slice、生命周期、零拷贝解析、整数溢出和 fuzz 思维。
- `Send`/`Sync`、channel、原子类型和背压。

**退出条件**

- 能准确解释“sample、frame、packet”的区别。
- 解析器面对随机或截断输入只返回错误，不 panic、不越界。
- 用测试证明环形缓冲区不会覆盖未消费数据。

### M2：Android 音频闭环（第 6–8 周）

**交付**

- Kotlin 选择本地 WAV，通过文件描述符交给 Rust。
- Rust 解析、工作线程供数、AAudio 回调消费 PCM。
- play、pause、stop、release 和播放位置。
- 输出设备断开后的错误处理与重建实验。

**学习**

- JNI 局部/全局引用、线程附着、字符串与文件描述符所有权。
- AAudio stream 状态、burst、underrun 和实时线程规则。
- RAII 封装原生句柄；为每个 unsafe 调用写安全契约。

**退出条件**

- 真机连续播放 30 分钟，无崩溃、无队列无限增长。
- 连续打开/释放 100 次，无明显 native 内存增长。
- 能观测 underrun，并用数据解释缓冲大小与延迟的权衡。

### M3：播放器核心（第 9–12 周）

**交付**

- 独立的 `moonegg-core`：状态机、command loop、event、clock。
- bounded queue、flush generation、EOS 和 seek 协议。
- virtual clock 与 fake backend，可在主机端确定性测试。
- 统一错误类型和 Android 错误映射。

**学习**

- 线程所有权、消息传递、取消、关闭顺序和死锁分析。
- 单调时钟、媒体时间、系统时间和 discontinuity。
- API 设计：同步命令、异步事件、可恢复错误与致命错误。

**退出条件**

- 状态迁移、重复命令、并发 release、seek 后旧帧丢弃均有测试。
- ThreadSanitizer 可覆盖的主机测试无数据竞争；所有线程可被 join。
- 核心 crate 在不链接 Android 的情况下完成测试。

### M4：压缩音频与媒体容器（第 13–15 周）

**交付**

- 接入 Android extractor/codec adapter，播放 MP4 中的 AAC 音轨。
- 处理 codec config、PTS、EOS、flush 和 seek-to-sync。
- PCM 格式变化适配；必要时加入最小重采样抽象。
- 媒体探测工具输出 tracks、codec、duration、time base。

**学习**

- container、track、sample、packet、codec config 和 time base。
- AAC frame、解码延迟、priming/padding 的基本概念。
- 平台 codec 的异步状态和 buffer ownership。

**退出条件**

- 至少覆盖 44.1/48 kHz、单/双声道和多种时长的测试文件。
- 随机执行 50 次 seek 后能够继续播放，位置误差有记录。
- 损坏媒体返回可读错误，不发生 native 崩溃。

### M5：视频与音画同步（第 16–20 周）

**交付**

- H.264 解码输出到 Surface，正确处理尺寸/旋转信息。
- 以音频为主时钟：视频早到则等待，轻微晚到立即显示，过晚则丢帧。
- pause/resume/seek 时统一 flush 音视频管线。
- Surface 销毁与重建、前后台切换、旋转屏幕测试。
- 同步偏差、丢帧数、首帧时间和队列水位指标。

**学习**

- YUV、色彩范围/空间、stride、crop、帧率与变帧率。
- PTS/DTS、B 帧、重排序、关键帧、seek 和 A/V sync。
- Android Surface、native window 与硬件解码生命周期。

**退出条件**

- 10 个受控样本完整播放；音画偏差目标 P95 小于 80 ms。
- 连续播放 60 分钟无崩溃；内存进入稳定区间后不持续增长。
- 旋转、切后台、锁屏返回、插拔耳机均有手工测试记录。

### M6：开源级 v0.1（第 21–24 周）

**交付**

- 固化 public API，补齐 API 文档和 Kotlin 示例。
- 基准测试、fuzz target、崩溃符号化和性能分析说明。
- 支持矩阵、已知限制、故障排查、架构图和贡献指南。
- 可复现的 v0.1 release、演示视频和技术文章。

**学习**

- SemVer、changelog、许可证合规和第三方 NOTICE。
- Android Perfetto、内存/CPU/线程分析和启动性能。
- Issue 拆分、code review、release checklist 和用户反馈闭环。

**退出条件**

- 一名不了解项目的开发者能只按 README 构建、运行、定位日志。
- CI 覆盖格式、lint、test、文档、Android 编译和基础 smoke test。
- v0.1 的支持范围、性能数据和限制都可被复现，而非口头描述。

## 5. 每周学习与开发节奏

推荐每周只围绕一个可验证问题工作：

- 60%：实现最小纵向切片与测试。
- 20%：阅读官方文档、规范或成熟实现，并记研究笔记。
- 10%：性能测量、故障注入或真机验证。
- 10%：写 ADR、周报或技术文章，回答“为什么这样设计”。

每周结束产出一份短记录：

```text
本周问题：
我建立的心智模型：
完成的可运行证据：
遇到的 bug 与根因：
测量结果：
仍不确定的地方：
下周唯一目标：
```

不要按“读完 Rust 书 → 学完音视频 → 再做项目”的顺序推进。每个知识点都应被当前里程碑立即使用；暂时用不到的内容不展开。

## 6. 能力地图与作品证据

| 能力 | 项目练习 | 可展示证据 |
|---|---|---|
| Rust 基础 | parser、状态机、错误类型 | 单测、文档测试、无 panic 输入处理 |
| 并发与资源管理 | command loop、有界队列、关闭协议 | 并发模型图、压力测试、泄漏数据 |
| unsafe / FFI | JNI、NDK 句柄封装 | safety comment、边界测试、ADR |
| 音频 | PCM、AAudio、underrun | 延迟/缓冲实验报告 |
| 视频 | codec、Surface、帧调度 | 真机演示、丢帧和首帧数据 |
| 音画同步 | clock、PTS、seek/flush | 同步算法说明、偏差分布 |
| 性能工程 | trace、benchmark、内存观测 | Perfetto 截图、前后对比表 |
| 开源协作 | issue、PR、release | changelog、贡献文档、稳定 tag |

## 7. 质量门槛

### 每个 PR

- 有明确问题和验收标准，尽量小于 400 行有效变更。
- 新行为有测试；bug fix 先有能复现问题的测试或样本。
- `cargo fmt`、Clippy、test 通过，不无理由 `allow` lint。
- 新增 unsafe 必须附安全不变量和审查说明。
- 行为或架构变化同步更新文档/ADR。

### 持续测试层级

1. 纯 Rust 单元测试：解析、状态机、时钟、队列。
2. 属性/fuzz 测试：不可信容器数据和操作序列。
3. 主机集成测试：fake source/codec/sink + virtual clock。
4. Android instrumented test：JNI、生命周期、资源释放。
5. 真机长稳测试：循环播放、随机 seek、前后台和 Surface 重建。

测试媒体必须小、可再生成、记录来源和许可证。不要把来源不明的影视文件提交到仓库。

## 8. 关键指标

第一天就定义指标，但到相应阶段再设置门槛：

- prepare 到首帧/首个音频的耗时。
- 当前播放位置与基准时钟偏差。
- 音画同步偏差的 P50/P95/max。
- 音频 underrun、视频 late/drop 数量。
- packet/frame 队列当前值与峰值。
- 长时间播放后的 native heap、线程数和句柄数。
- 100 次 create/prepare/release 的失败率与资源增长。

性能优化必须遵循“记录基线 → 提出假设 → 修改 → 同设备复测”，不接受只凭感觉的优化。

## 9. 重要决策清单

以下问题在进入对应阶段前用 ADR 回答：

1. Kotlin API 是 callback、Flow 还是轮询快照？
2. 公共 FFI 使用 JNI 直出还是内部稳定 C ABI + JNI adapter？
3. AAudio 直接绑定还是经由 Oboe；最低 Android 版本是否仍为 26？
4. extractor/codec 使用同步还是异步接口？各线程拥有哪个资源？
5. seek 的精度语义和超时/取消语义是什么？
6. Surface 消失时继续音频、暂停，还是进入 waiting-for-surface？
7. 何时引入 FFmpeg 软件后端，它解决的是兼容性还是跨平台？

默认建议是“内部 C ABI + JNI adapter、AAudio 直连、同步 codec 接口起步”，但必须通过最小原型和数据确认，不把建议当永久结论。

## 10. 风险控制

| 风险 | 早期信号 | 应对 |
|---|---|---|
| 范围失控 | 同时做网络、字幕、倍速 | 每个版本只保留一个主链路，非目标写入 README |
| 被构建系统拖住 | 数周仍无法播放声音 | 先 WAV + 平台 API，第三方大依赖延后 |
| Rust 只做胶水 | 核心逻辑全在 Kotlin/C++ | 状态机、时钟、调度、队列和资源模型归 Rust |
| 为“纯 Rust”重复造轮子 | 开始自研复杂 codec | 自研 parser 仅限学习样本，生产 codec 使用成熟实现 |
| 真机 bug 难复现 | 只有散乱日志 | 命令/状态/PTS/队列指标结构化，可导出诊断快照 |
| demo 能跑但不可维护 | 没有测试、release 卡死 | 每阶段设置自动化退出条件，不用演示代替验证 |
| 学得多但讲不清 | 只有代码没有解释 | 每个里程碑至少一篇设计或复盘文档 |

## 11. 下一步：第一个两周迭代

### Week 1：最小工程闭环

1. 确认许可证、包名、最低 Android API 和支持 ABI。
2. 建立 Rust workspace 与 `moonegg-core`。
3. 建立 Android Demo，并让 Kotlin 成功调用 Rust `version()`。
4. 加 CI、format、Clippy、test。
5. 写 ADR-0001（范围/架构）和 ADR-0002（FFI 边界）。

### Week 2：可测试的 WAV 解析

1. 只支持 PCM 16-bit little-endian 的最小 WAV 子集。
2. 先写有效、截断、未知 chunk、错误长度等测试。
3. 输出 `AudioFormat`、duration 和 PCM frame iterator。
4. 加第一份 fuzz target 或等价的随机输入属性测试。
5. 写一篇短文：从文件字节到 PCM frame 的数据路径。

两周后的演示不要求“像播放器”，只要求做到：一条命令跑完测试；Android App 能证明 FFI 闭环；解析器面对不可信输入保持安全。这是后续所有播放能力的可信地基。

## 12. v0.1 之后

只有 v0.1 达标后再按用户价值和学习价值排序：

1. v0.2：网络点播、缓冲策略、超时与取消。
2. v0.3：字幕、倍速、音频焦点、MediaSession/后台播放。
3. v0.4：FFmpeg 软件后端或桌面参考后端。
4. v0.5：HLS/DASH、自适应码率和更完整的可观测性。

DRM、直播低延迟、HDR 和专业音频能力应作为独立专题，不和基础播放器并行开工。
