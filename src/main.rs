mod position;

#[tokio::main]
async fn main() {
    let filename = "example_data.json";
    let positions = position::portfolio_position::from_file(filename);

    // move tasks into the async closure passed to tokio::spawn()
    let tasks: Vec<_> = positions
        .into_iter()
        .map(move |mut position| {
            tokio::spawn(async move {
                position::portfolio_position::handle_position(&mut position).await;
            })
        })
        .collect();

    for task in tasks {
        task.await.unwrap();
    }
}
