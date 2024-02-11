use crate::action::ActionType;
use crate::post::TopPost;
use crate::task::Task;
use crate::util::*;
use crate::workers::card::Card;
use crate::Commands::Cards;

pub fn create_context(post_top: TopPost, task: Task) -> Result<RenderingContext> {
    println!("Creating render.html and *.png cards");
    let card_post_index = match task.command {
        Cards {
            replies,
            reactions,
            forwards,
            views,
        } => [replies - 1, reactions - 1, forwards - 1, views - 1],
        _ => panic!("Wrong command"),
    };

    let get_post = |action: ActionType| &post_top.index(action)[card_post_index[action as usize]];
    let cards = vec![
        Card {
            header: String::from("Лучший по комментариям"),
            icon: icon_url("💬"),
            ..Card::create_card(get_post(ActionType::Replies), ActionType::Replies)
        },
        Card {
            header: String::from("Лучший по реакциям"),
            icon: icon_url("👏"),
            ..Card::create_card(get_post(ActionType::Reactions), ActionType::Reactions)
        },
        Card {
            header: String::from("Лучший по репостам"),
            icon: icon_url("🔁"),
            filter: String::from("filter-blue"),
            ..Card::create_card(get_post(ActionType::Forwards), ActionType::Forwards)
        },
        Card {
            header: String::from("Лучший по просмотрам"),
            icon: icon_url("👁️"),
            filter: String::from("filter-blue"),
            ..Card::create_card(get_post(ActionType::Views), ActionType::Views)
        },
    ];
    let cards: Vec<Card> = cards.into_iter().filter(|c| c.count.is_some()).collect();

    let mut context = RenderingContext::new();
    context.insert("cards", &cards);
    context.insert("editor_choice_id", &task.editor_choice_post_id);
    context.insert("channel_name", &task.channel_name.as_str());

    Ok(context)
}
