## 'static 的常见用法
先从一个trait的定义开始了解：
```rust
pub trait AudioOutputFactory: Send + Sync + 'static {
    // ...
}
```
意思是：
> 任何实现`AudioOutputFactory`的类型，都必须能安全地在线程间转移、能安全地跨线程共享引用、并且不依赖短生命周期的借用。
这里的约束作用于工厂类型本身，也就是`Self`
| 部分 | 含义 |
|---|---|
| `pub` | 允许其他模块或 crate 使用这个接口 |
| `Send` | 工厂值可以安全地转移到另一个线程 |
| `Sync` | 工厂的共享引用可以安全地被多个线程使用 |
| `'static` | 工厂内部不能依赖某个即将失效的局部借用 |

其中最容易误解的是： `T : 'static` 不代表这个值必须一直活到程序结束。

### 1. 引用的生命周期

```rust
let name: &'static str = "Android";
```
这里的`'static`修饰引用，表示它指向的数据在程序运行期间始终有效。字符串字面量可以满足这个条件。

### 2. 类型约束

```rust
T: 'static
```

表示：
> `T` 不包含需要依赖短生命周期数据才能保持有效的借用。
这里的 trait 就是第二种。

例如，`String`满足`'static`:
```rust
fn accepts_static<T: 'static>(value: T) {
    // value 可以在函数结束时正常销毁
}

fn main() {
    let name = String::from("Android");
    accepts_static(name);
}
```

### 'static 用法区分
```
&'static str
    → 这个引用指向的数据长期有效

String: 'static
    → 这个类型不依赖短期借用
    → 这个 String 值仍然可以随时正常销毁
```
