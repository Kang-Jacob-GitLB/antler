//! 항목 수와 총 비용(바이트 추정치)을 함께 제한하는 LRU 캐시(`src/main/services/lru.ts`). 값은 불변으로 취급한다(커밋 해시 기준 캐시용).
//! 항목이 수백 개뿐이라 가장 오래 안 쓴 항목은 사용 순번을 훑어 찾는다.

use std::collections::HashMap;

pub struct LruCache<V> {
    map: HashMap<String, Entry<V>>,
    max_entries: usize,
    max_cost: u64,
    total: u64,
    /// 사용할 때마다 늘어나는 순번. 작을수록 오래전에 쓴 것이다
    tick: u64,
}

struct Entry<V> {
    value: V,
    cost: u64,
    used: u64,
}

impl<V: Clone> LruCache<V> {
    pub fn new(max_entries: usize, max_cost: u64) -> Self {
        LruCache { map: HashMap::new(), max_entries, max_cost, total: 0, tick: 0 }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn cost(&self) -> u64 {
        self.total
    }

    /// 찾으면 최근 사용으로 올린다
    pub fn get(&mut self, key: &str) -> Option<V> {
        self.tick += 1;
        let tick = self.tick;
        let e = self.map.get_mut(key)?;
        e.used = tick;
        Some(e.value.clone())
    }

    /// cost 가 `max_cost` 를 혼자 넘는 항목은 저장하지 않는다(다른 항목을 다 밀어내지 않도록)
    pub fn set(&mut self, key: &str, value: V, cost: u64) {
        self.delete(key);
        if cost > self.max_cost {
            return;
        }
        self.tick += 1;
        self.map.insert(key.to_string(), Entry { value, cost, used: self.tick });
        self.total += cost;
        while self.map.len() > self.max_entries || self.total > self.max_cost {
            let Some(oldest) = self.map.iter().min_by_key(|(_, e)| e.used).map(|(k, _)| k.clone()) else { break };
            self.delete(&oldest);
        }
    }

    pub fn delete(&mut self, key: &str) -> bool {
        match self.map.remove(key) {
            Some(e) => {
                self.total -= e.cost;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 항목_수_상한을_넘으면_가장_오래_안_쓴_것부터_버린다() {
        let mut c = LruCache::new(2, 100);
        c.set("a", 1, 1);
        c.set("b", 2, 1);
        assert_eq!(c.get("a"), Some(1)); // a 가 최근
        c.set("c", 3, 1);
        assert_eq!(c.get("b"), None);
        assert_eq!(c.get("a"), Some(1));
        assert_eq!(c.get("c"), Some(3));
    }

    #[test]
    fn 비용_상한을_넘으면_버리고_혼자_넘는_항목은_저장하지_않는다() {
        let mut c = LruCache::new(10, 10);
        c.set("a", 1, 6);
        c.set("b", 2, 6); // 합 12 > 10 → a 가 밀려난다
        assert_eq!(c.get("a"), None);
        assert_eq!(c.cost(), 6);
        c.set("huge", 3, 11);
        assert_eq!(c.get("huge"), None);
        assert_eq!(c.len(), 1);
        // 같은 키를 다시 넣으면 비용이 갱신된다
        c.set("b", 4, 2);
        assert_eq!(c.cost(), 2);
        assert_eq!(c.get("b"), Some(4));
    }
}
