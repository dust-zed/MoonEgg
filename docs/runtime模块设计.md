## 模块主线

开始说明之前，先记住一条主线：
```
ControlLoop：决定应该做什么
    ↓
run_effect_loop：接收决定，调用执行器
    ↓
PlaybackEffectExecutor：创建、指挥、关闭播放 worker
    ↓
PlaybackWorker：实际操作 pipeline，持续推进播放
    ↓
WorkerEvent：把结果交回 ControlLoop
```
当前主要有 Control、Effect、Playback 三条线程，其中`run_effect_loop`和 `PlabackEffectExecutor`在同一条 Effect 线程上运行。

### effect_executor.rs ： 把 Control 的结果送去执行

这个文件主要包含三部分：
| 组成 | 职责 |
|---|---|
| `EffectExecutor` | 规定执行器应提供什么接口 |
| `run_effect_loop()` | 接收 Control 的结果，转发事件并执行 effect |
| `EffectFeedback` | 提供带 epoch 的反馈通道 |
先看接口：
```rust
fn execute(
    &mut self,
    effect: ControlEffect,
    feedback: &EffectFeedback
)
```
它表达的是：
> 各你一项 Control 已经批准的工作，以及一个报告结果的入口，请执行它。

接口本身不限定如何执行。对于当前播放器，一些 effect 会创建线程，一些会发送命令，一些会关闭线程。

`run_effect_loop()` 的核心流程是：
```
等待 ControlResult
    ↓
如果命令被拒绝，向外发送 CommandRejected
    ↓
取得 ControlOutcome
    ↓
向外转发其中的 PlayerEvent
    ↓
如果存在 ControlEffect：
    创建对应 epoch 的 EffectFeedback
    调用 executor.execute()
```
这里为什么同时处理 event 和 effect？
因为 Control 的一次决策，可能产生两个结果：
```
“状态进入 Playing” → 告知外部调用者

“启动音频输出” → 交给执行器落实
```
它们分别对应“对外通知”和“执行工作”。

`EffectFeedback` 则封装：
```rust
epoch: PlaybackEpoch,
sender: Sender<ControlMessage>,
```
worker 通过它报告：
- 准备完成；
- 播放进度；
- 播放完成；
- 执行失败。
绑定 epoch 的意义，是让反馈保留产生它时所属的播放代次。Control 收到旧代次的结果，就可以忽略，避免 Seek 之前的结果影响到 Seek 之后的状态。
执行失败也需要返回 Control，由 Control 决定进入 `Error`、更新 epoch、安排清理。执行器没有修改播放器状态的权限。

---

### worker.rs: 提供线程生命周期的基础工具

这个文件没有持续解码的主循环。它提供的是工作线程通用的管理能力：
| 类型 | 用途 |
|---|---|
| `CancellationToken` | 共享的取消请求标记 |
| `WorkerHandle` | 持有取消令牌和线程 `JoinHandle` |
| `WorkerGroup` | 批量取消、等待多个线程退出 |
| `WorkerEvent` | 工作线程向 Control 报告的结果 |

其中最重要的区分：

```
cancel()：请求线程停止

join()：等待线程实际结束
```
取消是协作式的。设置标记后，需要 worker 主动检查并退出，不能强行中断任意阻塞操作。

`WorkerHandle` 将两项能力放在一起：
```rust
struct WorkerHandle {
    cancel: AtomicBool,
    thread: JoinHandle<()>,
}
```
持有这个 handle 的一方负责管理线程，但线程里实际处理什么业务，留给具体 worker。
`WorkerGroup`的关闭顺序也有设计含义：
```
先向全部线程发出取消请求
    ↓
再逐个 join
```
这样不会在等待第一个线程时，让其他线程还不知道应该退出。

---
### playback_executor.rs: 管理一次播放会话的 worker
这个文件实现的是具体执行器：
```rust
struct PlaybackEffectExecutor<F> {
    factory: Arc<F>,
    worker: Option<PlaybackWorkerHandle>,
}
```
两个字段分别表示：
```
factory: 知道如何创建播放 pipeline
worker: 当前播放会话的工作线程句柄
```
`Option`表达会话是否存在：
```
None
    → 尚未准备，或已经停止并清理

Some(handle)
    → 存在一个可以管理的播放 worker
```
它对 effect 的处理可以直接对应到一张表：
| ControlEffect | 执行器的操作 |
|---|---|
| `BeginPrepare` | 创建播放 worker，交给它构建 pipeline |
| `StartPlayback` | 向 worker 发送 `Start` |
| `PausePlayback` | 向 worker 发送 `Pause` |
| `Seek` | 向 worker 发送带目标位置的 `Seek` |
| `Stop` / `Release` / `CleanupAfterFailure` | 取出 worker，取消并 join |
注意，`BeginPrepare` 的执行成功，只说明 worker 成功启动，并不代表文件已经准备完成。
实际准备过程发生在 worker 内：
```
创建线程成功
    ↓
worker 调用 factory.build()
    ↓
打开文件、创建 demuxer / decoder / output
    ↓
报告 PreparationCompleted
    ↓
Control 才进入 Ready
```
这让耗时的准备工作和持续的媒体处理由 Playback 线程承担，Effect 线程负责调度。
不过，Effect 线程也并非完全不阻塞：当前 Stop、Release 的实现会等待 worker 退出。这个等待保证后续会话不会在
旧 worker 尚未关闭时就开始创建。

`worker.take()` 在这里非常关键：
```
从 Option 中取出句柄
    ↓
执行器立即变成 None
    ↓
使用取出的句柄完成 shutdown
```
所有权明确转移，后续清理就不会再次拿到同一个线程句柄。
---
### playback_worker.rs： 实际运行 pipeline 的地方
这个文件可以分成“线程内部”和"线程外部"两侧理解。
| 线程内部 | 线程外部 |
|---|---|
| `PlaybackWorker` | `PlaybackWorkerHandle` |
| 持有 pipeline | 持有命令发送端 |
| 接收并执行命令 | 提供 `send()` |
| 持续推进播放 | 提供 `shutdown()` |

外部拿到的是控制入口，pipeline 本身由 worker 独占。
因此，当前 demuxer、decoder、output 都在同一条 Playback 线程上使用，调用链是：
```
worker
    → PlaybackPipeline
        → demuxer
        → AudioPipeline
            → decoder
            → output
```
目前并没有“一条 demux 线程、一条 decode 线程、一条 output 线程”。这是后续可以评估的拆分方式，不是当前实现。

#### 它的第一个设计重点：在工作线程内部创建资源。
`spawn_playback_worker()`接收一个构建闭包，然后在线程内调用它：
```
Effect 线程传入构建闭包
    ↓
Playback 线程开始运行
    ↓
在线程内创建 pipeline
    ↓
在线程内使用并释放 pipeline
```
跨线程移动的是闭包及其捕获的数据。所以闭包要求 `Send`,而这里的 demuxer、decoder、output类型没有同一要求`Send`。
这个边界也为以后接入线程使用约束平台资源留出了空间。

#### 第二个重点：把播放拆成一小步一小步。

主循环大致是：
```
检查取消
    ↓
接收命令
    ↓
执行 Start / Pause / Seek
    ↓
定期报告媒体位置
    ↓
检查自然播放结束
    ↓
允许继续生产数据时，执行 pipeline.step()
    ↓
根据本轮结果决定是否等待
```

为什么使用`step()`？
因为 worker 在两次推进之间可以重新检查命令和取消标记。这样持续解码不会把暂停、Seek、关闭操作一直挡在后面。
