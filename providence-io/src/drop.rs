pub fn defer(cb: impl FnOnce()) -> impl Drop {
    struct Dropper<F: FnOnce()>(Option<F>);
    impl<F: FnOnce()> Drop for Dropper<F> {
        fn drop(&mut self) {
            self.0.take().unwrap()();
        }
    }

    Dropper(Some(cb))
}
