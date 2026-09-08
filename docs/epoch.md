## 含义
Epoch 可以理解为：
> 给当前一整套工作状态分配一个“世代编号”， 结果回来时先检查它不属于当前世代
它解决的不是数据排序问题，而是异步工作过期问题

## Seek竞态
假设播放器当前：
```
epoch = 3
正在播放 10 秒附近
```
Decoder worker 已经接到任务：
```
任务：
epoch = 3
解码 PTS = 10.040s 的 Packet
```
这时用户 seek 到 60 秒：
```
current_epoch：3 -> 4
```
随后开始读取和解码 60 秒附近的数据。
但是旧 worker 可能无法瞬间停止：
```
时间 ─────────────────────────────→

worker：开始解码旧 Packet ─────────── 解码完成
                    │
用户：              └── seek 到 60s，epoch 变为 4
```
旧结果回来时：
```
frame.epoch = 3
current_epoch = 4 
```
因此可以判断：
```
3 != 4
→ 这是上一条播放时间线的数据
→ 禁止进入当前 pipeline
```

### 为何只清空队列不够
清空队列只能处理“仍然放在队列里的数据”：
```
Packet Queue：
[A] [B] [C]
    ↓ clear
全部清空
```
但 worker 已经取走的 Packet 不再队列里：
```
Packet Queue       Decoder Worker
[A] [B]             正在处理 C
   ↓ clear              ↓
A、B 被清除         C 以后仍然会产生 Frame
```
Epoch 就是用来识别这个迟到的`C`

## Epoch的核心工作模式
整个思想只有两步。

### 启动工作时记录 epoch
```rust
let task_epoch = current_epoch;
```
把它附加到任务：
```
EpochItem {
  epoch: 3,
  value: Packet,
}
```
**提交结果前重新验证**
```rust
if result.epoch() != current_epoch {
    // 结果已经过期
    discard(result);
    return;
}
```
这时一种“先允许工作执行，提交结果前检查是否仍然有效”的思路。

## Epoch 不是取消机制
Epoch 只能保证：
> 旧工作的结果不会污染新状态。
它不保证：
> 旧工作立即停止执行。
例如旧 Decoder 任务可能仍会浪费一些 CPU，最后才发现结果过期。
因此成熟系统通常同时使用：
```
Cancellation
    → 尽快停止旧工作，节省资源

Epoch validation
    → 即使取消来不及，旧结果也不能生效
```
二者不是替代关系，而是两层保护。

## Epoch为什么很适合消息传递
不一定需要所有线程共享一个 AtomicU64。
可以由 ControlLoop 独占当前 epoch：
```
ControlLoop
├── current_epoch = 4
├── 发送任务(epoch=4)
└── 接收结果(epoch=4)
```
worker 只负责原样携带：
```
收到 Packet(epoch=4)
       ↓
执行解码
       ↓
返回 Frame(epoch=4)
```
最终只有 ControlLoop判断：
```
结果 epoch == current epoch?
```
这样避免所有线程共同修改全局状态。

## 资源仍然必须正确释放
旧结果不能展示，但不能简单忘掉：
```
旧 Packet      → 正常释放内存
旧 AudioBuffer → 正常释放 PCM
旧视频 token   → 归还 MediaCodec buffer
```
Epoch 决定“还能不能生效”，所有权决定“过期后如何安全销毁”。

## Epoch 所解决的问题
Epoch 是以下问题的对策：
> 任务开始时有效，不代表任务结束时仍然有效。
Epoch 就是对“是否仍然属于当前世界”的证明。
