## Demuxer 的职责
Demuxer 的职责只有一句话：
> 解析媒体容器，发现轨道，并按容器中的读取顺序输出压缩 Packet。

例如 MP4:
```
MP4 文件字节
    ↓
Demuxer
    ├── TrackInfo：H.264 视频轨
    ├── TrackInfo：AAC 音频轨
    └── Packet：属于某一条轨道的压缩数据
```
它不负责：
* 解码 H.264/AAC；
* 音视频同步；
* 把时间戳转换成系统时钟；
* 渲染；
* 决定 Packet 何时播放。

### 读取结果的表达
```
Packet       成功读出一个完整 Packet
NotReady     当前暂时没有足够数据
EndOfStream  媒体流已经正常结束
```

## 读取顺序、DTS 和 PTS

以一个包含 B 帧的简化 GOP 为例，画面希望这样播放：
```
I0 → B1 → B2 → P3
```
但是解码 B 帧需要同时参考前后的参考帧。要解码`B1`和`B2`，必须得到未来的`P3`，所以压缩数据需要这样送进解码器：
```
I0 → P3 → B1 → B2
```
因此有两种时间：
* DTS：这个 Packet 什么时候进入解码器；
* PTS：解码后的画面什么时候展示。

## Demuxer的 Packet 输出顺序
基本的数据通路应该是:
```
容器读取顺序
    ↓
Packet DTS/解码依赖顺序
    ↓
Decoder
    ↓
DecodedFrame
    ↓
根据 PTS 决定展示时间
```
### 多轨道发生交错
例如 MP4 内通常不是先存完所有视频，再存所有音频，而是类似：
```
Video Packet
Audio Packet
Audio Packet
Video Packet
Audio Packet
...
```
因此`dexmuer::read_packet()`的全局结果可能是：
```
V0 → A0 → A1 → V1 → A2 → V2
```
读取后根据`TrackId`分发：
```
┌→ video packet queue → video decoder
Demuxer → Packet ──┤
└→ audio packet queue → audio decoder
```

## Seek的完整流程
假设目标是 10 秒，前一个关键帧在 8 秒：
```
1. pipeline 进入 seeking 状态
2. 清空旧 Packet 队列
3. 清空旧 Frame 队列
4. decoder.flush()
5. demuxer.seek(10s) → 返回 8s
6. 从 8s 开始按读取/DTS 顺序提交 Packet
7. 解码 Frame
8. 丢弃 pts < 10s 的 Frame
9. 从目标附近恢复播放
```

## 媒体数据链核心语义

```
TrackInfo 描述轨道
Packet 表达压缩数据及 DTS/PTS
Demuxer 产生 Packet
Decoder 消费 Packet、产生 DecodedFrame
DecodedFrame<T> 持有软件数据或硬件 token
```
