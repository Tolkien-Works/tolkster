// The one way a lock is taken: a panic while one was held leaves what it guards as the panicking
// thread left it, and every holder writes that state whole, so a poisoned lock's guard is used
// as it is rather than taking `listen` down with it.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

pub fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn read<T: ?Sized>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

pub fn write<T: ?Sized>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

pub fn wait<'a, T>(changed: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    changed.wait(guard).unwrap_or_else(PoisonError::into_inner)
}
