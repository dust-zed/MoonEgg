## 基本概念
- Atomic: 保证一次操作的原子性，允许多个线程安全地操作同一个原子变量。
- Ordering：规定这次原子操作与其他内存访问之间的同步关系。

## Atomic的原子性，保证到哪里？
假设两个线程同时给计数器加一：
```
初始值：0

线程 A：读取 0
线程 B：读取 0
线程 A：写入 1
线程 B：写入 1

最终值：1
```
两次加一丢失了一次。
即使换成原子变量，下面这样写，仍然可能出现同样的问题：
```rust
let value = counter.load(Ordering::Relaxed);
counter.store(value + 1, Ordering::Relaxed);
```
因为这是两次独立的原子操作，其他线程可以在它们之间修改计数值。
需要使用整体的“读取-修改-写入”操作：
```rust
counter.fetch_add(1, Ordering::Relaxed);
```
这样整个加一过程具有原子性。忽略整数溢出的情况，两个线程各执行一次，最终结果会增加二。
常见原子类型包括`AtomicBool`，`AtomicUsize`。它们可以通过共享引用操作；跨线程共享所有权时，经常搭配`Arc`。

## 为什么已经有 Atomic，还需要 Ordering？

考虑一组操作：
```
生产者：
① 写入数据
② 标记“数据准备好了”

消费者：
③ 看到“准备好了”
④ 读取数据
```
我们需要的关系是：
```
看到准备完成 → 能够读取到发布前写好的数据
```
这里涉及两个位置： 数据和标记。
单独保证标记的读写具有原子性，还不足以建立这个关系。编译器优化、处理器执行和内存可见性，都必须遵守你声明的同步约束。
`Ordering` 就是在表达这些约束。
不要把它理解成“每次强制刷新 CPU 缓存”。学习时先围绕线程之间允许观察到什么结果来推理。

## 五种 Ordering

| Ordering | 主要用途 |
|---|---|
| `Relaxed` | 只需要该变量本身的原子操作，不借它发布其他数据 |
| `Release` | 发布此前完成的写入 |
| `Acquire` | 接收匹配的发布，使后续读取获得同步保证 |
| `AcqRel` | 用于同时读取和写入的操作，兼具获取与发布语义 |
| `SeqCst` | 在相应获取、发布保证之外，为所有 `SeqCst` 操作建立统一的全序 |

`Release` 用于能够写入的操作，`Acquire`用于能够读取的操作；`AcqRel` 用于读改写操作，不能用于普通`load`或`store`。

### 用一个例子理解Release/Acquire

假设共享变量初始为：
```rust
let data = AtomicUsize::new(0);
let ready = AtomicBool::new(false);
```
下面是一轮发布过程：只有一个生产者写入，发布后不会再修改数据。
生产者线程执行：
```rust
data.store(42, Ordering::Relaxed);
ready.store(true, Ordering::Release);
```
消费者线程执行:
```rust
if ready.load(Ordering::Acquire) {
    let value = data.load(Ordering::Relaxed);
    assert_eq!(42, value);
}
```
这里成立的关系是：
```
data 写入 42
    ↓ 生产者线程内的先后关系
ready.store(true, Release)
    ↓ 消费者读取到了这次发布的 true
ready.load(Acquire)
    ↓ 消费者线程内的先后关系
读取 data
```
因此，进入`if`后，断言成立。
注意三个条件：
- `Release` 和 `Acquire` 操作的是同一个同步变量`Ready`
- 消费者读到了相应发布操作的写入值。
- 示例中没有其他线程继续改写`data`。

如果 `ready` 的两次访问都使用`Relaxed`，就没有上述发布关系，不能仅凭 `ready == true` 断言 `data == 42`。
这里把`data`也写成了原子变量，是为了让示例本身保持安全；真实项目可以通过通道、锁等更高层接口完成数据交接。
