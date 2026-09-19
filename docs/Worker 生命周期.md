## worker 生命周期
worker生命周期最重要的是分清四件事：
> 创建线程、持有管理句柄、请求退出、等待线程结束。
它们发生在不同位置，也不一定同时完成。
当前播放 worker 的生命周期可以先画成：
```
Prepare
   ↓
创建线程
   ↓
在线程内构建 pipeline
   ↓
准备完成，等待命令
   ↓
播放 / 暂停 / Seek / 自然结束
   ↓
Stop、Release 或失败清理
   ↓
请求取消
   ↓
worker 清理资源并退出
   ↓
管理方 join，确认线程结束
```
### 1. 谁持有什么？
先看管理侧的所有权：
```
Effect 线程
└── PlaybackEffectExecutor
    └── Option<PlaybackWorkerHandle>
        ├── 命令发送端
        └── Option<WorkerHandle>
            ├── CancellationToken
            └── JoinHandle<()>
```
再看实际工作的线程：
```
Playback 线程
└── PlaybackWorker
    ├── pipeline
    ├── 命令接收端
    ├── CancellationToken
    └── EffectFeedback
```
这里最容易混淆的是：
- `PlaybackWorker` 是线程内部执行工作的对象。
- `PlaybackWorkerHandle` 是外部管理这个线程的入口。
- `JoinHandle` 用于等待线程结束，并取得线程是否 panic 的结果。
**外部句柄并没有直接持有 pipeline**。pipeline 在线程内部创建、使用和释放。

两个`CancellationToken` 通过`Arc<AtomicBool>`共享同一个取消标记。

### 2. 创建成功和准备完成，是两个时刻

收到`BeginPrepare`后，执行器调用：
```
spawn_playback_worker(...)
```

这个函数先准备：
```
命令通道
取消令牌
构建闭包
```
然后创建线程。
创建成功后，外部拿到`PlaybackWorkerHandle`，执行器把它保存到：

```rust
self.worker = Some(worker)
```
但此时线程内部可能仍然在执行：

```rust
build(epoch, &thread_cancel)
```
所以存在两个独立结果：
| 阶段 | 成功意味着什么 | 失败怎样报告 |
|---|---|---|
| 创建线程 | 得到了线程管理句柄 | `spawn_playback_worker()` 返回错误 |
| 构建 pipeline | 媒体和组件准备完成 | worker 发送 `Failed` 反馈 |

准备完成后，worker 报告`PreparationCompleted`，Control 才进入`Ready`。
这也解释了：**执行器持有 Some(handle), 不代表媒体已经准备好，更不代表线程始终健康。**它表示执行器仍然持有这次会话的管理责任。

### 3. 播放状态变化，不等于线程生命周期变化
当前实现中，一条 worker 线程可以经历：

```
Ready → Playing → Paused → Playing → Ended
                    ↓
                  Seek
                    ↓
              Paused → Playing
```
这些变化通常都在同一条线程上完成。
| 操作或事件 | worker 线程 | pipeline |
|---|---|---|
| Pause | 保留 | 保留数据，暂停输出 |
| Seek | 保留 | 清理旧数据并重新定位 |
| 自然播放结束 | 保留 | 保留，允许后续 Seek |
| Stop | 退出并 join | 释放 |
| Release | 退出并 join | 释放 |
因此，**一首音频播放结束，worker 不会立即销毁。**
保留线程和 pipeline，才能在`Ended`后 Seek，再次播放。
Stop 完成后再次 Prepare，则会重新创建新的 worker、新的命令通道和新的取消令牌。

### 4. 取消只是请求，退出由 worker 自己完成
`WorkerHandle::cancel()`最终只是：

```rust
self.cancelled.store(true, Ordering::Relaxed);
```
它不直接销毁线程，也不会强行执行线程内部的清理代码。
worker 在检查点读取：

```rust
if self.cancel.is_canceled() {
    break;
}
```

然后离开循环，执行：
```rust
self.close_pipeline()
```

当前`close_pipeline()`做的是：
```
清除运行标记
    ↓
从 Option 中取出 pipeline
    ↓
尝试暂停输出
    ↓
释放 pipeline
```
这就是协作式取消： 管理方表达意图，工作线程在自己的执行上下文里完成退出。

### 5. 为什么cancel之后还需要 join？
因为下面两个时刻不同：
- 取消标记已经设为 true
- worker 已经清理完资源并结束

`join()`用于等待后者。

`WorkerHandle::join(self)`消耗自身所有权，也表达了一个约束：这个线程的等待权被使用后，不能再使用同一个 handle 重复 join。

### 6. Option::take() 怎样避免重复关闭
这里实际有两层`take()`。
第一层在执行器中：
```rust
let Some(worker) = self.worker.take() else {
    return Ok(());
}
worker.shutdown();
```

它把会话管理句柄取走：
```
执行前: Some(handle)
执行后：None
```
执行器不再保留一个正在关闭或已经关闭的会话。
第二层在`PlaybackWorkerHandle::stop_worker()`中：
```rust
let Some(worker) = self.worker.take() else {
    return Ok(());
};
worker.cancel();
worker.join()
```
它取走真正包含`JoinHandle`的对象，确保取消和 join 只执行一次。
因此显式关闭的过程是：
```
shutdown(self)
    ↓
stop_worker()
    ↓
take() 取走底层 WorkerHandle
    ↓
cancel + join
    ↓
shutdown 返回，self 被销毁
    ↓
Drop 再调用 stop_worker()
    ↓
此时是 None，直接返回
```
`stop_worker()` 可能被调用两次，但第二次没有线程句柄可处理。
这正是`Option`在这里表达的资源状态:还有没有尚未处理的关闭责任。

### 7. 显式shutdown和Drop，各自承担什么？

当前代码同时提供：
```rust
pub(crate) fn shutdown(mut self) -> Result<(), PlaybackError>
```
和
```rust
impl Drop for PlaybackWorkerHandle {
    fn drop(&mut self) {
        let _ = self.stop_worker();
    }
}
```

两者的区别主要在错误处理：
| 方式 | 用途 |
|---|---|
| 显式 `shutdown()` | 正常关闭路径，可以把 join 失败返回上层 |
| `Drop` | 句柄被意外丢弃时，尝试取消并等待，避免遗留后台线程 |

`Drop`没有返回`Result`的入口，所以这里忽略关闭结果。
还有一个工程上的含义：这个 handle 的析构可能阻塞，因为它会 join。不能把它理解成一个随时丢弃、立即返回的普通小对象。

### 8. 播放失败和线程退出，也要分开
正常播放错误，例如解码失败，会走：
```
worker 清理 pipeline
    ↓
发送 WorkerEvent::Failed
    ↓
Control 生成 CleanupAfterFailure
    ↓
执行器取消并 join worker
```

worker 报告失败后，可以暂时继续接收已经排队的命令，因此会出现：
```
线程仍然存在
pipeline = None
```
它不再处理媒体，但仍能给后续命令返回明确失败结果，直到管理方完成关闭。
线程 panic 则不同。它可能没有机会按正常流程发送`Failed`；管理方在 join 时会发现线程异常结束，映射成`WorkerPanicked`。

因此要分别观察：
```
WorkerEvent::Failed
    → 播放业务是否失败

join 的结果
    → 线程是否正常返回
```
同样，join 成功不代表媒体播放成功。线程可以先报告解码失败，再正常退出。
当前`close_pipeline()`还会忽略`pause()`的错误，因此 join 成功也不能证明每个清理调用都成功了。

### 9. 整个引擎关闭时，worker 处于哪一层？

从调用方看：
```
PlayerEngine::shutdown()
    ↓
发送 Release 给 Control
    ↓
Control 生成 Release effect
    ↓
Effect 线程执行 Release
    ↓
关闭并 join Playback worker
    ↓
Effect 线程退出
    ↓
引擎取得 Effect 和 Control 的退出结果
```
引擎当前先 join effect 线程，再 join Control 线程。这样正常路径下，等待 Effect 返回，也包含了等待它所管理的播放 worker 关闭。
但这不意味着 Control 一定最后结束：Control 发出 Release 的结果后就可以退出，底层资源清理仍可能在进行。等待顺序与线程实际结束顺序是两回事。

在这个设计里，一次播放会话的关闭责任属于`PlaybackEffectExecutor`；整个播放器的关闭责任属于`PlayerEngine`。

10. WorkerGroup 为以后多个 worker 提供什么
当前播放主链路管理的是单个 worker；WorkerGroup 还没有承担这条链路的管理工作。
它的关闭策略是：
```
先取消全部 worker
    ↓
再逐个 join
    ↓
即使其中一个失败，仍继续等待剩余 worker
    ↓
最后返回记录的第一个错误
```
这避免在等待第一个线程时，其他线程还没有收到取消请求；也避免因为某个线程 panic，就跳过其他线程的回收。
以后如果拆出多个工作线程，除了沿用这个管理方式，还要明确它们之间的等待关系：被取消的 worker 必须能走到退出点，管理方的 join 才能完成。
