//! Price-time priority limit order book.
//!
//! Features:
//! - Limit orders (buy/sell) with partial fills
//! - Cancel by order ID
//! - Trade execution with maker/taker
//! - Order book depth queries
//! - Deterministic matching

use std::collections::{BTreeMap, VecDeque};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Price(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OrderId(pub u64);

#[derive(Debug, Clone)]
pub struct Order {
    pub id: OrderId,
    pub side: Side,
    pub price: Price,
    pub quantity: u64,
    pub filled: u64,
    pub timestamp: u64,
}

impl Order {
    pub fn remaining(&self) -> u64 {
        self.quantity - self.filled
    }
    pub fn is_filled(&self) -> bool {
        self.filled >= self.quantity
    }
}

#[derive(Debug, Clone)]
pub struct Trade {
    pub maker_order_id: OrderId,
    pub taker_order_id: OrderId,
    pub price: Price,
    pub quantity: u64,
    pub timestamp: u64,
}

#[derive(Debug)]
pub enum OrderBookError {
    OrderNotFound,
    OrderAlreadyFilled,
    InvalidQuantity,
    InvalidPrice,
}

impl fmt::Display for OrderBookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderBookError::OrderNotFound => write!(f, "order not found"),
            OrderBookError::OrderAlreadyFilled => write!(f, "order already filled"),
            OrderBookError::InvalidQuantity => write!(f, "invalid quantity"),
            OrderBookError::InvalidPrice => write!(f, "invalid price"),
        }
    }
}

/// Price-time priority order book.
pub struct OrderBook {
    bids: BTreeMap<Price, VecDeque<Order>>, // price -> FIFO queue
    asks: BTreeMap<Price, VecDeque<Order>>,
    orders: std::collections::HashMap<OrderId, Order>,
    next_id: u64,
    timestamp: u64,
    trades: Vec<Trade>,
}

impl OrderBook {
    pub fn new() -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            orders: std::collections::HashMap::new(),
            next_id: 1,
            timestamp: 0,
            trades: Vec::new(),
        }
    }

    pub fn place_order(&mut self, side: Side, price: Price, quantity: u64) -> Result<OrderId, OrderBookError> {
        if quantity == 0 { return Err(OrderBookError::InvalidQuantity); }
        if price.0 == 0 { return Err(OrderBookError::InvalidPrice); }

        self.timestamp += 1;
        let id = OrderId(self.next_id);
        self.next_id += 1;

        let mut order = Order {
            id,
            side,
            price,
            quantity,
            filled: 0,
            timestamp: self.timestamp,
        };

        // Match against opposite side
        match side {
            Side::Buy => self.match_buy(&mut order),
            Side::Sell => self.match_sell(&mut order),
        }

        if !order.is_filled() {
            self.orders.insert(order.id, order.clone());
            let book = match side {
                Side::Buy => &mut self.bids,
                Side::Sell => &mut self.asks,
            };
            book.entry(price).or_default().push_back(order);
        }

        Ok(id)
    }

    fn match_buy(&mut self, taker: &mut Order) {
        while taker.remaining() > 0 {
            let best_ask_price = match self.asks.keys().next() {
                Some(p) => *p,
                None => break,
            };
            if best_ask_price > taker.price {
                break;
            }

            let queue = self.asks.get_mut(&best_ask_price).unwrap();
            if queue.is_empty() {
                self.asks.remove(&best_ask_price);
                continue;
            }

            let maker = queue.front_mut().unwrap();
            let trade_qty = taker.remaining().min(maker.remaining());

            maker.filled += trade_qty;
            taker.filled += trade_qty;

            self.trades.push(Trade {
                maker_order_id: maker.id,
                taker_order_id: taker.id,
                price: best_ask_price,
                quantity: trade_qty,
                timestamp: self.timestamp,
            });

            if maker.is_filled() {
                let filled_maker = queue.pop_front().unwrap();
                self.orders.remove(&filled_maker.id);
            }
        }
        // Clean empty price levels
        self.asks.retain(|_, q| !q.is_empty());
    }

    fn match_sell(&mut self, taker: &mut Order) {
        while taker.remaining() > 0 {
            let best_bid_price = match self.bids.keys().next_back() {
                Some(p) => *p,
                None => break,
            };
            if best_bid_price < taker.price {
                break;
            }

            let queue = self.bids.get_mut(&best_bid_price).unwrap();
            if queue.is_empty() {
                self.bids.remove(&best_bid_price);
                continue;
            }

            let maker = queue.front_mut().unwrap();
            let trade_qty = taker.remaining().min(maker.remaining());

            maker.filled += trade_qty;
            taker.filled += trade_qty;

            self.trades.push(Trade {
                maker_order_id: maker.id,
                taker_order_id: taker.id,
                price: best_bid_price,
                quantity: trade_qty,
                timestamp: self.timestamp,
            });

            if maker.is_filled() {
                let filled_maker = queue.pop_front().unwrap();
                self.orders.remove(&filled_maker.id);
            }
        }
        self.bids.retain(|_, q| !q.is_empty());
    }

    pub fn cancel_order(&mut self, id: OrderId) -> Result<Order, OrderBookError> {
        let order = self.orders.remove(&id).ok_or(OrderBookError::OrderNotFound)?;
        if order.is_filled() {
            return Err(OrderBookError::OrderAlreadyFilled);
        }

        let book = match order.side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };

        if let Some(queue) = book.get_mut(&order.price) {
            queue.retain(|o| o.id != id);
            if queue.is_empty() {
                book.remove(&order.price);
            }
        }

        Ok(order)
    }

    pub fn best_bid(&self) -> Option<Price> {
        self.bids.keys().next_back().copied()
    }

    pub fn best_ask(&self) -> Option<Price> {
        self.asks.keys().next().copied()
    }

    pub fn spread(&self) -> Option<u64> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some(ask.0 - bid.0),
            _ => None,
        }
    }

    pub fn bid_depth(&self) -> usize {
        self.bids.values().map(|q| q.len()).sum()
    }

    pub fn ask_depth(&self) -> usize {
        self.asks.values().map(|q| q.len()).sum()
    }

    pub fn trades(&self) -> &[Trade] {
        &self.trades
    }

    pub fn order(&self, id: OrderId) -> Option<&Order> {
        self.orders.get(&id)
    }
}

impl Default for OrderBook {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_place_buy() {
        let mut book = OrderBook::new();
        let id = book.place_order(Side::Buy, Price(100), 10).unwrap();
        assert_eq!(book.best_bid(), Some(Price(100)));
        assert_eq!(book.bid_depth(), 1);
        assert!(book.order(id).is_some());
    }

    #[test]
    fn test_place_sell() {
        let mut book = OrderBook::new();
        let id = book.place_order(Side::Sell, Price(100), 10).unwrap();
        assert_eq!(book.best_ask(), Some(Price(100)));
        assert_eq!(book.ask_depth(), 1);
        assert!(book.order(id).is_some());
    }

    #[test]
    fn test_simple_match() {
        let mut book = OrderBook::new();
        book.place_order(Side::Sell, Price(100), 10).unwrap();
        let buy_id = book.place_order(Side::Buy, Price(100), 10).unwrap();
        assert_eq!(book.trades().len(), 1);
        assert_eq!(book.trades()[0].quantity, 10);
        assert_eq!(book.trades()[0].price, Price(100));
    }

    #[test]
    fn test_partial_fill() {
        let mut book = OrderBook::new();
        book.place_order(Side::Sell, Price(100), 10).unwrap();
        book.place_order(Side::Buy, Price(100), 5).unwrap();
        assert_eq!(book.trades().len(), 1);
        assert_eq!(book.trades()[0].quantity, 5);
        assert_eq!(book.ask_depth(), 1); // sell order still has 5 remaining
    }

    #[test]
    fn test_no_match_price_gap() {
        let mut book = OrderBook::new();
        book.place_order(Side::Sell, Price(110), 10).unwrap();
        book.place_order(Side::Buy, Price(100), 10).unwrap();
        assert_eq!(book.trades().len(), 0);
        assert_eq!(book.bid_depth(), 1);
        assert_eq!(book.ask_depth(), 1);
        assert_eq!(book.spread(), Some(10));
    }

    #[test]
    fn test_cancel_order() {
        let mut book = OrderBook::new();
        let id = book.place_order(Side::Buy, Price(100), 10).unwrap();
        let cancelled = book.cancel_order(id).unwrap();
        assert_eq!(cancelled.id, id);
        assert_eq!(book.bid_depth(), 0);
        assert!(book.order(id).is_none());
    }

    #[test]
    fn test_cancel_filled_fails() {
        let mut book = OrderBook::new();
        book.place_order(Side::Sell, Price(100), 10).unwrap();
        let buy_id = book.place_order(Side::Buy, Price(100), 10).unwrap();
        assert!(book.cancel_order(buy_id).is_err());
    }

    #[test]
    fn test_multiple_price_levels() {
        let mut book = OrderBook::new();
        book.place_order(Side::Sell, Price(100), 5).unwrap();
        book.place_order(Side::Sell, Price(101), 5).unwrap();
        book.place_order(Side::Sell, Price(102), 5).unwrap();
        // Buy should match best (lowest) ask first
        book.place_order(Side::Buy, Price(102), 7).unwrap();
        assert_eq!(book.trades().len(), 2);
        assert_eq!(book.trades()[0].price, Price(100));
        assert_eq!(book.trades()[1].price, Price(101));
    }

    #[test]
    fn test_price_time_priority() {
        let mut book = OrderBook::new();
        // Two sells at same price
        let id1 = book.place_order(Side::Sell, Price(100), 5).unwrap();
        let id2 = book.place_order(Side::Sell, Price(100), 5).unwrap();
        // Buy should fill id1 first (time priority)
        book.place_order(Side::Buy, Price(100), 5).unwrap();
        assert_eq!(book.trades()[0].maker_order_id, id1);
    }

    #[test]
    fn test_invalid_inputs() {
        let mut book = OrderBook::new();
        assert!(book.place_order(Side::Buy, Price(0), 10).is_err());
        assert!(book.place_order(Side::Buy, Price(100), 0).is_err());
        assert!(book.cancel_order(OrderId(999)).is_err());
    }

    #[test]
    fn test_best_bid_ask() {
        let mut book = OrderBook::new();
        assert_eq!(book.best_bid(), None);
        assert_eq!(book.best_ask(), None);
        book.place_order(Side::Buy, Price(99), 10).unwrap();
        book.place_order(Side::Buy, Price(100), 10).unwrap();
        book.place_order(Side::Sell, Price(101), 10).unwrap();
        book.place_order(Side::Sell, Price(102), 10).unwrap();
        assert_eq!(book.best_bid(), Some(Price(100)));
        assert_eq!(book.best_ask(), Some(Price(101)));
    }

    #[test]
    fn test_large_order_sweep() {
        let mut book = OrderBook::new();
        for i in 0..5 {
            book.place_order(Side::Sell, Price(100 + i), 10).unwrap();
        }
        book.place_order(Side::Buy, Price(105), 30).unwrap();
        assert_eq!(book.trades().len(), 3);
        assert_eq!(book.ask_depth(), 2); // 2 remaining sell orders
    }
}
