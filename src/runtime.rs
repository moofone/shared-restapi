use std::future::Future;

pub fn block_on_rest_future<T, E, F, M>(
    future: F,
    operation: impl Into<String>,
    map_runtime_error: M,
) -> Result<T, E>
where
    T: Send + 'static,
    E: Send + 'static,
    F: Future<Output = Result<T, E>> + Send + 'static,
    M: Fn(String) -> E + Copy + Send + 'static,
{
    let operation = operation.into();
    if tokio::runtime::Handle::try_current().is_ok() {
        return std::thread::spawn(move || {
            run_on_new_runtime(future, operation, map_runtime_error)
        })
        .join()
        .map_err(|panic_payload| {
            let message = if let Some(msg) = panic_payload.downcast_ref::<&str>() {
                (*msg).to_string()
            } else if let Some(msg) = panic_payload.downcast_ref::<String>() {
                msg.clone()
            } else {
                "unknown panic payload".to_string()
            };
            map_runtime_error(format!("shared-restapi worker thread panicked: {message}"))
        })?;
    }

    run_on_new_runtime(future, operation, map_runtime_error)
}

fn run_on_new_runtime<T, E, F, M>(
    future: F,
    operation: String,
    map_runtime_error: M,
) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
    M: Fn(String) -> E,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| {
            map_runtime_error(format!(
                "failed to initialize shared-restapi runtime for {operation}: {err}"
            ))
        })?;
    runtime.block_on(future)
}
