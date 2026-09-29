### 1. 接入思路：让 Android 实现适配 core，而不是让 core 围绕 Android 重写
core 规定播放器需要哪些能力：
```
Demuxer: 读取编码包、查询轨道、seek
Decoder：提交编码包、接收解码结果、flush
AudioOutput: 提交 PCM、控制输出、查询位置与排空状态
```

Android 提供具体实现：
```
flowchart LR
    A["文件 / fd 区间"] --> B["AndroidDemuxer<br/>MediaExtractor"]
    B -->|"Packet：编码数据 + PTS"| C["AndroidAudioDecoder<br/>MediaCodec"]
    C -->|"DecodedFrame：PCM + PTS"| D["AndroidAudioOutput<br/>AAudio"]
```
播放管线通过 trait 使用这些对象，负责推进数据、处理背压和协调结束流程。Android 实现负责把平台行为翻译为这些
 trait 的返回值。
 这里形成了一个重要习惯：先确定接口契约，再封装平台 API。
 例如，MediaCodec 暂时没有输入缓冲区，应该转换成`Backpressure`；暂时没有输出，应该转换成`NotReady`。这
 两者都不是播放失败。

 ### 2. 分层封装：底层管理平台资源，上层表达媒体语义
目前的实现采用了两层结构：
| 底层封装 | 上层适配 | 分工 |
|---|---|---|
| `NativeMediaExtractor` | `AndroidDemuxer` | 前者管理句柄、调用平台；后者识别 AAC 轨道、构造 Packet |
| `NativeMediaCodec` | `AndroidAudioDecoder` | 前者管理缓冲区与状态；后者处理轨道、时间戳、PCM 格式 |
| `AAudioPcmStream` | `AndroidAudioOutput` | 前者封装设备操作；后者管理待写 PCM、背压和输出契约 |
这让`unsafe`、原始指针和平台返回码主要集中在底层。上层看到的是 Rust 的对象、枚举和 Result。

比如“取出一个 MediaCodec 输出缓冲区”和“把这块内容解释成一个 PCM 帧”，是两个不同的责任。分开以后，边界检查和媒体处理就
不会挤在同一个函数里。

### 3.工厂的价值：把创建方式、创建时机和对象使用分开
`AndroidAudioBackendFactory` 保存媒体来源，提供创建解复用器和解码器的能力：
```rust
// audio_backend_factory.rs
fn open_demuxer(&self) -> Result<Self::Demux, DemuxError>;

fn create_audio_decoder(
    &self,
    track: &TrackInfo,
) -> Result<Self::Decode, DecodeError>;
```
它们合在一起，是因为共同组成输入侧的处理方案。输出工厂单独存在，是因为它依赖 PCM 格式，可以被不同输入
方案复用。
core 内部的 `BackendPlaybackFactory`再将两种工厂组合起来，负责轨道选择、取消检查和管线创建。
创建顺序因此变得明确：
```
| 时机 | 当前创建的对象 |
|---|---|
| 构造播放器 | 工厂、控制与执行运行时 |
| Prepare | 解复用器、解码器、延迟输出包装器 |
| 首个非空 PCM 到达 | 实际音频输出设备 |
```
**工厂创建成功，不代表它将来创建的所有资源都会成功**。所以打开文件失败、解码器创建失败、输出设备创建失败，可能
在不同阶段上报。

### 4. Android解复用：从媒体文件得到可解码的Packet
目前已经实践了这条过程：
```
设置 fd、offset、length
    → 枚举并探测轨道
    → 选择 AAC 轨道
    → 读取样本数据和时间戳
    → 构造 Packet
    → advance 到下一份样本
```
这里掌握了几项具体知识：
- 容器与编码不同：M4A 是文件组织形式，AAC 是其中音频轨道的编码。
- 轨道信息与编码包不同：轨道信息提供编码配置；Packet 提供逐包数据及时间信息。
- csd-0 是解码初始化配置，需要供轨道信息传递给解码器。
- Packet 可以带 PTS，不需要等解码出 PCM 才知道时间。
- seek 请求位置与实际落点可能不同，因此接口返回实际落点。
- Android 提供的微秒时间戳，需要明确时间基，再转换到 core 的时间表示。

fd 区间也很重要：asset 可能位于 APK 文件内部，不能只传 fd 而忽略 offset、length。

### 5.MediaCodec解码：重点是缓冲区协议和状态管理。
解码并不是简单的：
```
输入一个 Packet → 立即返回一个 PCM
```

目前的接口已经把提交和接收分开：
```
submit：尝试交给解码器
receive：尝试取出解码结果
```
输入侧要处理：
- 暂时没有槽位。
- 编码数据超过槽位容量。
- 编码配置与普通数据的区别。
- 输入 EOS。
- 提交失败后的状态。

输出侧要处理：
- 暂时没有结果
- 输出格式变化通知
- 有效 PCM 数据
- 输出 EOS
- 取得的输出槽位必须归还

尤其值得记住两项实践。
第一，有效数据长度与缓冲区容量不同。复制前检查容量，复制时只复制本次有效长度。
第二，平台内存不能在归还缓冲区之后继续借用。当前实现将输出复制到 Rust 拥有的 Vec，再释放平台槽位，让后续管线处理不依赖该槽位的生命周期。

### 6.格式处理：输入轨道格式与实际PCM格式各司其职
`AudioTrackFormat`用于描述输入和配置解码器；`AudioPcmFormat`用于解释解码后的采样数据、创建输出及建立音频时钟。

现在通过解码器的输出格式通知，确认 PCM 的：
- 采样率
- 声道数
- 样本类型，例如 I16 或 F32。
`DeferredAudioOutput`等首个非空 PCM 到达再创建设备，并保存提前收到的播放意图。

这并不意味着格式可以任意改变。当前实现会拒绝设备创建后的格式变化；AAudio 封装也会检查实际打开的流是否符合请求。
还有一个具体能力边界：PCM 解析层支持 I16、F32，而当前 Android 输出只支持 I16。 接口能表示某种格式，不代表整个播放链路都已支持它。
