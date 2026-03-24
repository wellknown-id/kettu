# Resources

Resources are handle types that represent external objects with lifetimes.

## Defining Resources

```kettu
interface filesystem {
    resource file {
        constructor(path: string);
        
        read: func(bytes: u32) -> list<u8>;
        write: func(data: list<u8>);
        close: func();
        
        size: static func(path: string) -> u64;
    }
}
```

## Resource Methods

### Constructor

Creates a new resource instance:

```kettu
constructor(path: string);
constructor(name: string, size: u32);
```

### Instance Methods

Called on a resource instance (implicit `self`):

```kettu
read: func(bytes: u32) -> list<u8>;
write: func(data: list<u8>);
```

### Static Methods

Called on the resource type, not an instance:

```kettu
exists: static func(path: string) -> bool;
create: static func(path: string) -> file;
```

## Resource Handles

Resources are passed by handle:

```kettu
interface processor {
    use filesystem.{file};
    
    process: func(f: file);
    
    // Ownership transfer
    consume: func(f: own<file>);
    
    // Borrowing
    inspect: func(f: borrow<file>);
}
```

## Example

```kettu
interface storage {
    resource blob {
        constructor(data: list<u8>);
        
        len: func() -> u64;
        slice: func(start: u64, end: u64) -> list<u8>;
        
        empty: static func() -> blob;
    }
}
```
