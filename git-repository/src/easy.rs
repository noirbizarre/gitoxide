//! ### Which `Easy*` is for me?
//!
//! * Use `Easy*Exclusive` when the underlying `Repository` eventually needs mutation, for instance to update data structures
//!    - This is useful for long-running applications that eventually need to adapt to changes in the repository and pick up
//!      new packs after a GC operation or a received pack.
//! * Use the non-exclusive variants if the `Repository` doesn't ever have to change, for example as in one-off commands.
//!
//! ### Implementation Notes
//!
//! - Why no `Easy` with simply an owned `Repository`, instead `Rc<Repository>` is enforced
//!    - When this is desired, rather use `EasyShared` and drop the `EasyShared` once mutable access to the `Repository` is needed.
//!      `Access` is not usable for functions that require official `&mut` mutability, it's made for interior mutability to support
//!       trees of objects.
use std::{
    cell::RefCell,
    ops::{Deref, DerefMut},
};

use crate::{odb, refs, Repository};

type PackCache = odb::pack::cache::Never; // TODO: choose great all-round cache

#[derive(Default)]
pub struct State {
    packed_refs: RefCell<Option<refs::packed::Buffer>>,
    pack_cache: RefCell<PackCache>,
    buf: RefCell<Vec<u8>>,
}

pub trait Access {
    type RepoRef: Deref<Target = Repository>;
    // TODO: Once GATs become stable, try to use them to make it work with RefCells too, aka EasyExclusive
    type RepoRefMut: DerefMut<Target = Repository>;

    fn repo(&self) -> std::result::Result<Self::RepoRef, borrow::Error>;
    /// # NOTE
    ///
    /// This is implemented only for `EasyArcExclusive` to be obtained via `to_easy_arc_exclusive()`
    fn repo_mut(&self) -> std::result::Result<Self::RepoRefMut, borrow::Error>;
    fn state(&self) -> &State;
}

pub type Result<T> = std::result::Result<T, state::borrow::Error>;

pub mod borrow {
    use std::{
        cell::{BorrowError, BorrowMutError},
        fmt::{Display, Formatter},
    };

    #[derive(Debug)]
    pub struct Error;

    impl Display for Error {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            "Could not borrow the repository as it's already borrowed mutably".fmt(f)
        }
    }

    impl std::error::Error for Error {}

    impl From<BorrowError> for Error {
        fn from(_: BorrowError) -> Self {
            Error
        }
    }
    impl From<BorrowMutError> for Error {
        fn from(_: BorrowMutError) -> Self {
            Error
        }
    }
}

pub mod state {
    use std::{
        cell::{Ref, RefMut},
        ops::DerefMut,
    };

    use crate::{
        easy,
        easy::PackCache,
        refs,
        refs::{file, packed},
    };

    pub mod borrow {
        use quick_error::quick_error;
        quick_error! {
            #[derive(Debug)]
            pub enum Error {
                Borrow(err: std::cell::BorrowError) {
                    display("A state member could not be borrowed")
                    from()
                }
                BorrowMut(err: std::cell::BorrowMutError) {
                    display("A state member could not be mutably borrowed")
                    from()
                }
            }
        }
    }

    impl easy::State {
        // TODO: this method should be on the Store itself, as one day there will be reftable support which lacks packed-refs
        // TODO: provide a way to update a cache if the underlying pack changed or got deleted.
        pub(crate) fn assure_packed_refs_present(&self, file: &file::Store) -> Result<(), packed::buffer::open::Error> {
            if self.packed_refs.borrow().is_none() {
                *self.packed_refs.borrow_mut().deref_mut() = file.packed()?;
            }
            Ok(())
        }

        #[inline]
        pub(crate) fn try_borrow_packed_refs(&self) -> Result<Ref<'_, Option<refs::packed::Buffer>>, borrow::Error> {
            self.packed_refs.try_borrow().map_err(Into::into)
        }

        #[inline]
        pub(crate) fn try_borrow_mut_pack_cache(&self) -> Result<RefMut<'_, PackCache>, borrow::Error> {
            self.pack_cache.try_borrow_mut().map_err(Into::into)
        }

        #[inline]
        pub(crate) fn try_borrow_mut_buf(&self) -> Result<RefMut<'_, Vec<u8>>, borrow::Error> {
            self.buf.try_borrow_mut().map_err(Into::into)
        }

        #[inline]
        pub(crate) fn try_borrow_buf(&self) -> Result<Ref<'_, Vec<u8>>, borrow::Error> {
            self.buf.try_borrow().map_err(Into::into)
        }
    }
}

mod impls {
    use std::{rc::Rc, sync::Arc};

    use parking_lot::lock_api::{ArcRwLockReadGuard, ArcRwLockWriteGuard};

    use crate::{easy, Easy, EasyArc, EasyArcExclusive, EasyShared, Repository};

    impl Clone for Easy {
        fn clone(&self) -> Self {
            Easy {
                repo: Rc::clone(&self.repo),
                state: Default::default(),
            }
        }
    }

    impl Clone for EasyArc {
        fn clone(&self) -> Self {
            EasyArc {
                repo: Arc::clone(&self.repo),
                state: Default::default(),
            }
        }
    }

    impl<'repo> Clone for EasyShared<'repo> {
        fn clone(&self) -> Self {
            EasyShared {
                repo: self.repo,
                state: Default::default(),
            }
        }
    }

    impl From<Repository> for Easy {
        fn from(repo: Repository) -> Self {
            Easy {
                repo: Rc::new(repo),
                state: Default::default(),
            }
        }
    }

    impl From<Repository> for EasyArc {
        fn from(repo: Repository) -> Self {
            EasyArc {
                repo: Arc::new(repo),
                state: Default::default(),
            }
        }
    }

    impl From<Repository> for EasyArcExclusive {
        fn from(repo: Repository) -> Self {
            EasyArcExclusive {
                repo: Arc::new(parking_lot::RwLock::new(repo)),
                state: Default::default(),
            }
        }
    }

    impl Repository {
        pub fn to_easy(&self) -> EasyShared<'_> {
            EasyShared {
                repo: self,
                state: Default::default(),
            }
        }
        pub fn into_easy(self) -> Easy {
            self.into()
        }

        pub fn into_easy_arc(self) -> EasyArc {
            self.into()
        }

        pub fn into_easy_arc_exclusive(self) -> EasyArcExclusive {
            self.into()
        }
    }

    impl<'repo> easy::Access for EasyShared<'repo> {
        type RepoRef = &'repo Repository;
        type RepoRefMut = &'repo mut Repository;

        fn repo(&self) -> Result<Self::RepoRef, easy::borrow::Error> {
            Ok(self.repo)
        }

        fn repo_mut(&self) -> Result<Self::RepoRefMut, easy::borrow::Error> {
            Err(easy::borrow::Error)
        }

        fn state(&self) -> &easy::State {
            &self.state
        }
    }

    impl easy::Access for Easy {
        type RepoRef = Rc<Repository>;
        type RepoRefMut = ArcRwLockWriteGuard<parking_lot::RawRwLock, Repository>; // this is a lie

        fn repo(&self) -> Result<Self::RepoRef, easy::borrow::Error> {
            Ok(self.repo.clone())
        }

        fn repo_mut(&self) -> Result<Self::RepoRefMut, easy::borrow::Error> {
            Err(easy::borrow::Error)
        }

        fn state(&self) -> &easy::State {
            &self.state
        }
    }

    impl easy::Access for EasyArc {
        type RepoRef = Arc<Repository>;
        type RepoRefMut = ArcRwLockWriteGuard<parking_lot::RawRwLock, Repository>; // this is a lie

        fn repo(&self) -> Result<Self::RepoRef, easy::borrow::Error> {
            Ok(self.repo.clone())
        }
        fn repo_mut(&self) -> Result<Self::RepoRefMut, easy::borrow::Error> {
            Err(easy::borrow::Error)
        }
        fn state(&self) -> &easy::State {
            &self.state
        }
    }

    impl easy::Access for EasyArcExclusive {
        type RepoRef = ArcRwLockReadGuard<parking_lot::RawRwLock, Repository>;
        type RepoRefMut = ArcRwLockWriteGuard<parking_lot::RawRwLock, Repository>;

        fn repo(&self) -> Result<Self::RepoRef, easy::borrow::Error> {
            Ok(self.repo.read_arc())
        }
        fn repo_mut(&self) -> Result<Self::RepoRefMut, easy::borrow::Error> {
            Ok(self.repo.write_arc())
        }
        fn state(&self) -> &easy::State {
            &self.state
        }
    }
}