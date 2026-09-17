对于普通的 RIFF/WAV 文件，一般的结构是：
```
"RIFF" + 长度字段 + “WAVE” + 一系列子 chunk
```

其中，`“WAVE”`叫做格式类型标识（format type）,说明这个 RIFF 容器装的是 WAVE 音频内容。
| 部分 | 作用 |
|---|---|
| `"RIFF"` | 标识最外层是一个 RIFF 块 |
| 长度字段 | 说明后面的 `"WAVE"` 和所有子块总共有多长 |
| `"WAVE"` | 标识这个容器的具体类型是 WAVE |
| 子 chunks | 分别存放格式说明、音频数据、元数据等 |

还有一个细节：最外层的 RIFF 自己也是一个 chunk，其他 chunk 嵌套在它里面。
```
RIFF chunk
├── 类型标识："WAVE"
├── "fmt " chunk：音频格式说明
├── "data" chunk：实际音频数据
└── 其他 chunk：可选信息
```
`"WAVE"`只是外层内容开头的四个字节，它本身不是一个独立 chunk；`"fmt "`则有自己的块名、长度字段和数据内容。
