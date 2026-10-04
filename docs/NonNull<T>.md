## 核心定义
`NonNull<T>` 就是保证不是“空指针的原始指针”。

普通原始指针：
```rust
*mut T
```
允许为空，也允许非空。而:
```rust
NonNull<T>
```
要求它保存的地址一定非空。

原生接口创建对象时，经常通过空指针表示失败：
```rust
let raw = unsafe { ndk::sys::AMediaExtractor_new() };
```
此时`raw` 的类型是：
```rust
*mut ndk::sys::AMediaExtractor
```
我们可以在创建边界检查一次：
```rust
let inner = NonNull::new(raw)
    .ok_or(MediaExtractorError::CreateFailed)?;
```
这里：
- `raw`为空 -> `NonNull::new` 返回`None` -> 返回创建失败。
- `raw` 非空 -> 返回 `Some(NonNull<...>)` -> 保存到结构体中。
因此，只要 `NativeMediaExtractor` 构造成功，它的 `inner` 就一定不是空指针。后续方法不需要反复检查为空。

不过，非空只是一项保证，不等于指针安全有效。

`NonNull` 不保证：
- 指向对象还活着；
- 对象已经正确初始化；
- 当前线程可以访问它；
- 不存在非法的并发访问。

例如，原生对象删除后，之前的地址仍然可能非零，但它已经成为悬空指针了。
所以，`NonNull` 不能替代生命周期管理。

可以对比：
| 类型 | 保证非空 | 自带资源释放行为 |
|---|---:|---:|
| `*mut T` | 否 | 否 |
| `NonNull<T>` | 是 | 否 |
| `Box<T>` | 是 | 是，按 Rust 的所有权和分配规则释放 |

因此我们需要自己写 Drop。自动清理的能力来自 `NativeMediaExtractor` 的 `Drop`，并不是来自 `NonNull`。
