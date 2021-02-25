use std::collections::HashMap;
use std::convert::From;
use std::fs;

use diesel::prelude::*;
use itertools::{EitherOrBoth::*, Itertools};
use juniper::{graphql_object, FieldResult};

use crate::db::PooledConnection;
use crate::deck::graphql::CardGroup;
use crate::deck::records::{Card, CardGroupRecord, Deck};
use crate::errors::ServiceResult;
use crate::game::record::GameRecord;
use crate::game_rules::GameRules;
use crate::graphql::Context;
use crate::player::Player;
use crate::schema::{card_groups, card_groups_cards, cards, games_users, users};
use crate::user::model::User;

pub struct GameState<'a> {
    marker: std::marker::PhantomData<&'a ()>,
    inner: HashMap<User, HashMap<String, (CardGroupRecord, Vec<Card>)>>,
    game_id: i32,
}

impl<'a> GameState<'a> {
    pub fn players(self) -> Vec<Player<'a>> {
        let game_id = self.game_id;

        self.inner
            .into_iter()
            .map(|(user, card_group_details)| {
                let card_groups = card_group_details.into_iter().fold(
                    HashMap::new(),
                    |mut acc, (card_group_name, (card_group_record, cards))| {
                        acc.entry(card_group_name)
                            .or_insert(CardGroup::new(card_group_record, cards));

                        acc
                    },
                );

                Player::new(game_id, user, card_groups)
            })
            .collect()
    }
}

pub struct Game<'a> {
    marker: std::marker::PhantomData<&'a ()>,
    record: GameRecord,
}

impl<'a> Game<'a> {
    pub fn deal(&self, connection: &PooledConnection) -> ServiceResult<()> {
        let game_state = self.state(connection)?;
        let mut players = game_state.players();

        let yams = fs::read_to_string("poo_head_rules.yaml")
            .expect("Something went wrong reading the file");
        let game_rules: GameRules = serde_yaml::from_str(&yams).unwrap();
        let mut deck = Deck::new_from_description(game_rules.deck, connection)?;

        for player_hand in game_rules.player_hand.iter() {
            let mut hand_at_initial_deal_count_for_all_players = false;
            let mut player_card_groups: Vec<&mut CardGroup> = players
                .iter_mut()
                .map(|player| player.create_card_group_from_description(player_hand, connection))
                .fold_ok(Vec::new(), |mut acc, player| {
                    acc.push(player);
                    acc
                })?;

            for card_group_index in (0..player_card_groups.len()).cycle() {
                if card_group_index == 0 && hand_at_initial_deal_count_for_all_players {
                    break;
                } else {
                    hand_at_initial_deal_count_for_all_players = true
                }

                let card_group = player_card_groups
                    .get_mut(card_group_index)
                    .expect("Missing card_group fetched by index during dealing");
                let card_group_full =
                    card_group.deal_card_from_deck_if_not_full(&mut deck, connection)?;

                // If there aren't enough cards to finish dealing I abruptly
                // return here. Is this behavior correct? Should it be configurable?
                if deck.cards.len() == 0 {
                    return Ok(());
                }

                if !card_group_full {
                    hand_at_initial_deal_count_for_all_players = false;
                }
            }
        }

        Ok(())
    }

    pub fn state(&self, connection: &PooledConnection) -> ServiceResult<GameState> {
        let mut users = users::table
            .inner_join(games_users::table)
            .filter(games_users::game_id.eq(self.record.id))
            .order_by(users::id)
            .select(users::all_columns)
            .get_results::<User>(connection)?;

        let card_groups = CardGroupRecord::belonging_to(&users)
            .order_by(card_groups::user_id.desc())
            .load::<CardGroupRecord>(connection)?;

        let card_group_ids = &card_groups.iter().map(|c| c.id).collect::<Vec<i32>>();

        use diesel::pg::expression::dsl::any;
        let card_group_ids_and_cards = cards::table
            .inner_join(card_groups_cards::table)
            .filter(card_groups_cards::card_group_id.eq(any(card_group_ids)))
            .select((card_groups_cards::card_group_id, cards::all_columns))
            .order_by(card_groups_cards::card_group_id)
            .load::<(i32, Card)>(connection)?;

        let mut cards_by_card_group: HashMap<i32, Vec<Card>> = card_group_ids_and_cards
            .into_iter()
            .group_by(|(card_group_id, _)| *card_group_id)
            .into_iter()
            .fold(
                HashMap::new(),
                |mut acc, (card_group_id, card_group_cards_iter)| {
                    let card_group_cards = card_group_cards_iter.map(|(_, card)| card).collect();
                    acc.entry(card_group_id).or_insert(card_group_cards);
                    acc
                },
            );

        let mut card_groups_by_user = card_groups.grouped_by(&users);
        card_groups_by_user.reverse();

        let mut user_card_groups_cards: HashMap<
            User,
            HashMap<String, (CardGroupRecord, Vec<Card>)>,
        > = HashMap::new();
        for users_card_groups in card_groups_by_user.into_iter() {
            let user = users.pop().expect("Missing expected user");
            let user_card_groups = user_card_groups_cards.entry(user).or_insert(HashMap::new());

            for card_group in users_card_groups.into_iter() {
                let cards = cards_by_card_group
                    .remove(card_group.id())
                    .unwrap_or(Vec::new());
                user_card_groups.insert(card_group.name.clone(), (card_group, cards));
            }
        }

        Ok(GameState {
            game_id: self.record.id,
            inner: user_card_groups_cards,
            marker: std::marker::PhantomData,
        })
    }
}

#[graphql_object(context = Context<'a>)]
#[graphql(description = "A game")]
impl<'a> Game<'a> {
    fn id(&self) -> i32 {
        self.record.id
    }

    fn players(&self, context: &Context<'a>) -> FieldResult<Vec<Player>> {
        let connection = &context.db_pool.get()?;
        Ok(self.state(connection)?.players())
    }

    fn player_turn_index(&self) -> i32 {
        self.record.player_turn_index
    }
}

impl<'a> From<GameRecord> for Game<'a> {
    fn from(record: GameRecord) -> Game<'a> {
        return Game {
            marker: std::marker::PhantomData,
            record,
        };
    }
}
